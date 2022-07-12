//! Contains most of the boilerplate around testing audio processing.

use std::sync::atomic::Ordering;

use anyhow::{Context, Result};

use crate::host::Host;
use crate::plugin::audio_thread::process::{
    AudioBuffers, OutOfPlaceAudioBuffers, ProcessConfig, ProcessData,
};
use crate::plugin::ext::audio_ports::{AudioPortConfig, AudioPorts};
use crate::plugin::ext::note_ports::NotePorts;
use crate::plugin::instance::Plugin;
use crate::plugin::library::PluginLibrary;
use crate::tests::rng::{new_prng, NoteGenerator};
use crate::tests::TestStatus;

/// A helper to handle the boilerplate that comes with testing a plugin's audio processing behavior.
pub struct ProcessingTest<'a> {
    plugin: &'a Plugin<'a>,
    audio_buffers: AudioBuffers<'a>,
}

impl<'a> ProcessingTest<'a> {
    /// Construct a new processing test using out-of-place processing. This allocates the CLAP audio
    /// buffer structs needed for the test. Returns an error if the the inner vectors don't all have
    /// the same length.
    pub fn new_out_of_place(
        plugin: &'a Plugin<'a>,
        input_buffers: &'a mut [Vec<Vec<f32>>],
        output_buffers: &'a mut [Vec<Vec<f32>>],
    ) -> Result<Self> {
        Ok(Self {
            plugin,
            audio_buffers: AudioBuffers::OutOfPlace(OutOfPlaceAudioBuffers::new(
                input_buffers,
                output_buffers,
            )?),
        })
    }

    /// Run the standard audio processing test for a still **deactivated** plugin. This calls the
    /// process function `num_iters` times, and checks the output for consistency each time.
    ///
    /// The `Preprocess` closure is called before each processing cycle to allow the process data to be
    /// modified for the next process cycle.
    pub fn run<Preprocess>(
        &'a mut self,
        num_iters: usize,
        process_config: ProcessConfig,
        mut preprocess: Preprocess,
    ) -> Result<()>
    where
        Preprocess: FnMut(&mut ProcessData) -> Result<()> + Send,
    {
        self.plugin
            .host_instance
            .requested_restart
            .store(false, Ordering::SeqCst);

        let buffer_size = self.audio_buffers.len();
        let mut process_data = ProcessData::new(&mut self.audio_buffers, process_config);

        // If the plugin requests a restart in the middle of processing, then the plugin will be
        // stopped, deactivated, reactivated, and started again. Because of that, we need to keep
        // track of the number of processed iterations manually instead of using a for loop.
        let mut iters_done = 0;
        while iters_done < num_iters {
            self.plugin
                .activate(process_config.sample_rate, 1, buffer_size)?;

            self.plugin.on_audio_thread(|plugin| -> Result<()> {
                plugin.start_processing()?;

                // This test can be repeated a couple of times
                // NOTE: We intentionally do not disable denormals here
                'processing: while iters_done < num_iters {
                    iters_done += 1;

                    preprocess(&mut process_data)?;

                    // We'll check that the plugin hasn't modified the input buffers after the
                    // test
                    let original_input_buffers = process_data.buffers.inputs_ref().to_owned();

                    plugin
                        .process(&mut process_data)
                        .context("Error during audio processing")?;

                    // When we add in-place processing this will need some slightly different checks
                    match process_data.buffers {
                        AudioBuffers::OutOfPlace(_) => check_out_of_place_output_consistency(
                            &process_data,
                            &original_input_buffers,
                        ),
                    }
                    .with_context(|| {
                        format!(
                            "Failed during processing cycle {} out of {}",
                            iters_done + 1,
                            num_iters
                        )
                    })?;

                    process_data.clear_events();
                    process_data.advance_transport(buffer_size as u32);

                    // Restart processing as necesasry
                    if plugin
                        .host_instance()
                        .requested_restart
                        .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok()
                    {
                        log::trace!(
                            "Restarting the plugin during processing cycle {} out of {} after a \
                             call to 'clap_host::request_restart()'",
                            iters_done + 1,
                            num_iters
                        );
                        break 'processing;
                    }
                }

                plugin.stop_processing()
            })?;

            self.plugin.deactivate()?;
        }

        Ok(())
    }

    /// Run the standard audio processing test for a still **deactivated** plugin. This is identical
    /// to the [`run()`][Self::run()] function, except that it does exactly one processing cycle and
    /// thus non-copy values can be moved into the closure.
    pub fn run_once<Preprocess>(
        &'a mut self,
        process_config: ProcessConfig,
        preprocess: Preprocess,
    ) -> Result<()>
    where
        Preprocess: FnOnce(&mut ProcessData) -> Result<()> + Send,
    {
        self.plugin
            .host_instance
            .requested_restart
            .store(false, Ordering::SeqCst);

        let buffer_size = self.audio_buffers.len();
        let mut process_data = ProcessData::new(&mut self.audio_buffers, process_config);

        self.plugin
            .activate(process_config.sample_rate, 1, buffer_size)?;

        self.plugin.on_audio_thread(|plugin| -> Result<()> {
            plugin.start_processing()?;

            preprocess(&mut process_data)?;

            // We'll check that the plugin hasn't modified the input buffers after the
            // test
            let original_input_buffers = process_data.buffers.inputs_ref().to_owned();

            plugin
                .process(&mut process_data)
                .context("Error during audio processing")?;

            // When we add in-place processing this will need some slightly different checks
            match process_data.buffers {
                AudioBuffers::OutOfPlace(_) => {
                    check_out_of_place_output_consistency(&process_data, &original_input_buffers)
                }
            }
            .context("Failed during processing")?;

            process_data.clear_events();
            process_data.advance_transport(buffer_size as u32);

            plugin.stop_processing()
        })?;

        self.plugin.deactivate()
    }
}

/// The test for `ProcessingTest::BasicOutOfPlaceAudioProcessing`.
pub fn test_basic_out_of_place_audio_processing(
    library: &PluginLibrary,
    plugin_id: &str,
) -> TestStatus {
    let mut prng = new_prng();

    // The host doesn't need to do anything special for this test
    let host = Host::new();
    let result = library
        .create_plugin(plugin_id, host.clone())
        .context("Could not create the plugin instance")
        .and_then(|plugin| {
            plugin.init().context("Error during initialization")?;

            let audio_ports_config = match plugin.get_extension::<AudioPorts>() {
                Some(audio_ports) => audio_ports
                    .config()
                    .context("Error while querying 'audio-ports' IO configuration")?,
                None => {
                    return Ok(TestStatus::Skipped {
                        details: Some(String::from(
                            "The plugin does not support the 'audio-ports' extension.",
                        )),
                    })
                }
            };

            let (mut input_buffers, mut output_buffers) = audio_ports_config.create_buffers(512);
            ProcessingTest::new_out_of_place(&plugin, &mut input_buffers, &mut output_buffers)?
                .run(5, ProcessConfig::default(), |process_data| {
                    process_data.buffers.randomize(&mut prng);

                    Ok(())
                })?;

            // The `Host` contains built-in thread safety checks
            host.thread_safety_check()
                .context("Thread safety checks failed")?;

            Ok(TestStatus::Success { details: None })
        });

    match result {
        Ok(status) => status,
        Err(err) => TestStatus::Failed {
            details: Some(format!("{err:#}")),
        },
    }
}

/// The test for `ProcessingTest::BasicOutOfPlaceNoteProcessing`. This test is very similar to
/// `BasicAudioProcessing`, but it requires the `note-ports` extension, sends notes and/or MIDI to
/// the plugin, and doesn't require the `audio-ports` extension.
pub fn test_basic_out_of_place_note_processing(
    library: &PluginLibrary,
    plugin_id: &str,
) -> TestStatus {
    let mut prng = new_prng();

    let host = Host::new();
    let result = library
        .create_plugin(plugin_id, host.clone())
        .context("Could not create the plugin instance")
        .and_then(|plugin| {
            plugin.init().context("Error during initialization")?;

            // You can have note/MIDI-only plugins, so not having any audio ports is perfectly fine
            // here
            let audio_ports_config = match plugin.get_extension::<AudioPorts>() {
                Some(audio_ports) => audio_ports
                    .config()
                    .context("Error while querying 'audio-ports' IO configuration")?,
                None => AudioPortConfig::default(),
            };
            let note_port_config = match plugin.get_extension::<NotePorts>() {
                Some(note_ports) => note_ports
                    .config()
                    .context("Error while querying 'note-ports' IO configuration")?,
                None => {
                    return Ok(TestStatus::Skipped {
                        details: Some(String::from(
                            "The plugin does not implement the 'note-ports' extension.",
                        )),
                    })
                }
            };
            if note_port_config.inputs.is_empty() {
                return Ok(TestStatus::Skipped {
                    details: Some(String::from(
                        "The plugin implements the 'note-ports' extension but it does not have \
                         any input note ports.",
                    )),
                });
            }

            // We'll fill the input event queue with (consistent) random CLAP note and/or MIDI
            // events depending on what's supported by the plugin supports
            let mut note_event_rng = NoteGenerator::new(note_port_config);

            const BUFFER_SIZE: usize = 512;
            let (mut input_buffers, mut output_buffers) =
                audio_ports_config.create_buffers(BUFFER_SIZE);
            ProcessingTest::new_out_of_place(&plugin, &mut input_buffers, &mut output_buffers)?
                .run(5, ProcessConfig::default(), |process_data| {
                    note_event_rng.fill_event_queue(
                        &mut prng,
                        &process_data.input_events,
                        BUFFER_SIZE as u32,
                    )?;
                    process_data.buffers.randomize(&mut prng);

                    Ok(())
                })?;

            host.thread_safety_check()
                .context("Thread safety checks failed")?;

            Ok(TestStatus::Success { details: None })
        });

    match result {
        Ok(status) => status,
        Err(err) => TestStatus::Failed {
            details: Some(format!("{err:#}")),
        },
    }
}

/// The test for `ProcessingTest::InconsistentNoteProcessing`. This is the same test as
/// `BasicOutOfPlaceNoteProcessing`, but without requiring matched note on/off pairs and similar
/// invariants
pub fn test_inconsistent_note_processing(library: &PluginLibrary, plugin_id: &str) -> TestStatus {
    let mut prng = new_prng();

    let host = Host::new();
    let result = library
        .create_plugin(plugin_id, host.clone())
        .context("Could not create the plugin instance")
        .and_then(|plugin| {
            plugin.init().context("Error during initialization")?;

            let audio_ports_config = match plugin.get_extension::<AudioPorts>() {
                Some(audio_ports) => audio_ports
                    .config()
                    .context("Error while querying 'audio-ports' IO configuration")?,
                None => AudioPortConfig::default(),
            };
            let note_port_config = match plugin.get_extension::<NotePorts>() {
                Some(note_ports) => note_ports
                    .config()
                    .context("Error while querying 'note-ports' IO configuration")?,
                None => {
                    return Ok(TestStatus::Skipped {
                        details: Some(String::from(
                            "The plugin does not implement the 'note-ports' extension.",
                        )),
                    })
                }
            };
            if note_port_config.inputs.is_empty() {
                return Ok(TestStatus::Skipped {
                    details: Some(String::from(
                        "The plugin implements the 'note-ports' extension but it does not have \
                         any input note ports.",
                    )),
                });
            }

            // This RNG (Random Note Generator) allows generates mismatching events
            let mut note_event_rng =
                NoteGenerator::new(note_port_config).with_inconsistent_events();

            // TODO: Use in-place processing for this test
            const BUFFER_SIZE: usize = 512;
            let (mut input_buffers, mut output_buffers) =
                audio_ports_config.create_buffers(BUFFER_SIZE);
            ProcessingTest::new_out_of_place(&plugin, &mut input_buffers, &mut output_buffers)?
                .run(5, ProcessConfig::default(), |process_data| {
                    note_event_rng.fill_event_queue(
                        &mut prng,
                        &process_data.input_events,
                        BUFFER_SIZE as u32,
                    )?;
                    process_data.buffers.randomize(&mut prng);

                    Ok(())
                })?;

            host.thread_safety_check()
                .context("Thread safety checks failed")?;

            Ok(TestStatus::Success { details: None })
        });

    match result {
        Ok(status) => status,
        Err(err) => TestStatus::Failed {
            details: Some(format!("{err:#}")),
        },
    }
}

/// The process for consistency. This verifies that the output buffer doesn't contain any NaN,
/// infinite, or denormal values, that the input buffers have not been modified by the plugin, and
/// that the output event queue is monotonically ordered.
fn check_out_of_place_output_consistency(
    process_data: &ProcessData,
    original_input_buffers: &[Vec<Vec<f32>>],
) -> Result<()> {
    // The input buffer must not be overwritten during out of place processing, and the outputs
    // should not contain any non-finite or denormal values
    let input_buffers = process_data.buffers.inputs_ref();
    let output_buffers = process_data.buffers.outputs_ref();
    if input_buffers != original_input_buffers {
        anyhow::bail!(
            "The plugin has overwritten the input buffers during out-of-place processing"
        );
    }
    for (port_idx, channel_slices) in output_buffers.iter().enumerate() {
        for (channel_idx, channel_slice) in channel_slices.iter().enumerate() {
            for (sample_idx, sample) in channel_slice.iter().enumerate() {
                if !sample.is_finite() {
                    anyhow::bail!(
                        "The sample written to output port {port_idx}, channel {channel_idx}, and \
                         sample index {sample_idx} is {sample:?}"
                    );
                } else if sample.is_subnormal() {
                    anyhow::bail!(
                        "The sample written to output port {port_idx}, channel {channel_idx}, and \
                         sample index {sample_idx} is subnormal ({sample:?})"
                    );
                }
            }
        }
    }

    // If the plugin output any events, then they should be in a monotonically increasing order
    let mut last_event_time = 0;
    for event in process_data.output_events.events.lock().iter() {
        let event_time = event.header().time;
        if event_time < last_event_time {
            anyhow::bail!(
                "The plugin output an event for sample {event_time} after it had previously \
                 output an event for sample {last_event_time}"
            )
        }

        last_event_time = event_time;
    }

    Ok(())
}
