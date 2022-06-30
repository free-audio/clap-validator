//! Tests for individual plugin instances.

use anyhow::{Context, Result};
use clap::ValueEnum;
use std::process::Command;

use super::{TestCase, TestResult, TestStatus};
use crate::hosting::ClapHost;
use crate::plugin::audio_thread::process::{
    AudioBuffers, OutOfPlaceAudioBuffers, ProcessConfig, ProcessData,
};
use crate::plugin::ext::audio_ports::{AudioPortConfig, AudioPorts};
use crate::plugin::ext::note_ports::NotePorts;
use crate::plugin::instance::Plugin;
use crate::plugin::library::PluginLibrary;
use crate::tests::rng::{new_prng, NoteGenerator};

const BASIC_OUT_OF_PLACE_AUDIO_PROCESSING: &str = "process-audio-out-of-place-basic";
const BASIC_OUT_OF_PLACE_NOTE_PROCESSING: &str = "process-note-out-of-place-basic";
const INCONSISTENT_NOTE_PROCESSING: &str = "process-note-inconsistent";

/// The tests for individual CLAP plugins. See the module's heading for more information, and the
/// `description` function below for a description of each test case.
pub enum PluginTestCase {
    BasicOutOfPlaceAudioProcessing,
    BasicOutOfPlaceNoteProcessing,
    InconsistentNoteProcessing,
}

impl<'a> TestCase<'a> for PluginTestCase {
    /// A loaded CLAP plugin library and the ID of the plugin contained within that library that
    /// should be tested.
    type TestArgs = (&'a PluginLibrary, &'a str);

    const ALL: &'static [Self] = &[
        PluginTestCase::BasicOutOfPlaceAudioProcessing,
        PluginTestCase::BasicOutOfPlaceNoteProcessing,
        PluginTestCase::InconsistentNoteProcessing,
    ];

    fn from_str(string: &str) -> Option<Self> {
        match string {
            BASIC_OUT_OF_PLACE_AUDIO_PROCESSING => {
                Some(PluginTestCase::BasicOutOfPlaceAudioProcessing)
            }
            BASIC_OUT_OF_PLACE_NOTE_PROCESSING => {
                Some(PluginTestCase::BasicOutOfPlaceNoteProcessing)
            }
            INCONSISTENT_NOTE_PROCESSING => Some(PluginTestCase::InconsistentNoteProcessing),
            _ => None,
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            PluginTestCase::BasicOutOfPlaceAudioProcessing => BASIC_OUT_OF_PLACE_AUDIO_PROCESSING,
            PluginTestCase::BasicOutOfPlaceNoteProcessing => BASIC_OUT_OF_PLACE_NOTE_PROCESSING,
            PluginTestCase::InconsistentNoteProcessing => INCONSISTENT_NOTE_PROCESSING,
        }
    }

    fn description(&self) -> String {
        match self {
            PluginTestCase::BasicOutOfPlaceAudioProcessing => String::from("Processes random audio through the plugin with its default parameter values and tests whether the output does not contain any non-finite or subnormal values. Uses out-of-place audio processing."),
            PluginTestCase::BasicOutOfPlaceNoteProcessing => String::from("Sends audio and random note and MIDI events to the plugin with its default parameter values and tests the output for consistency. Uses out-of-place audio processing."),
            PluginTestCase::InconsistentNoteProcessing => String::from("Sends intentionally inconsistent and mismatching note and MIDI events to the plugin with its default parameter values and tests the output for consistency. Uses out-of-place audio processing."),
        }
    }

    fn set_out_of_process_args(&self, command: &mut Command, (library, plugin_id): Self::TestArgs) {
        let test_name = self.as_str();

        command
            .arg(
                crate::validator::SingleTestType::Plugin
                    .to_possible_value()
                    .unwrap()
                    .get_name(),
            )
            .arg(library.library_path())
            .arg(plugin_id)
            .arg(test_name);
    }

    fn run_in_process(&self, (library, plugin_id): Self::TestArgs) -> TestResult {
        let status = match self {
            PluginTestCase::BasicOutOfPlaceAudioProcessing => {
                let mut prng = new_prng();

                // The host doesn't need to do anything special for this test
                let host = ClapHost::new();
                let result = library
                    .create_plugin(plugin_id, host.clone())
                    .context("Could not create the plugin instance")
                    .and_then(|plugin| {
                        plugin.init().context("Error during initialization")?;

                        let audio_ports_config =
                            match plugin.get_extension::<AudioPorts>() {
                                Some(audio_ports) => audio_ports.config().context(
                                    "Error while querying 'audio-ports' IO configuration",
                                )?,
                                None => return Ok(TestStatus::Skipped {
                                    reason: Some(String::from(
                                        "The plugin does not support the 'audio-ports' extension.",
                                    )),
                                }),
                            };

                        // This test only uses out-of-place processing
                        let process_config = ProcessConfig {
                            sample_rate: 44_100.0,
                            tempo: 110.0,
                            time_sig_numerator: 4,
                            time_sig_denominator: 4,
                        };
                        let (mut input_buffers, mut output_buffers) =
                            audio_ports_config.create_buffers(512);
                        let audio_buffers = AudioBuffers::OutOfPlace(
                            OutOfPlaceAudioBuffers::new(&mut input_buffers, &mut output_buffers)
                                .unwrap(),
                        );

                        run_out_of_place_audio_processing_test(
                            &plugin,
                            audio_buffers,
                            process_config,
                            |process_data| {
                                process_data.buffers.randomize(&mut prng);

                                Ok(())
                            },
                        )?;

                        // The `ClapHost` contains built-in thread safety checks
                        host.thread_safety_check()
                            .context("Thread safety checks failed")?;

                        Ok(TestStatus::Success { notes: None })
                    });

                match result {
                    Ok(status) => status,
                    Err(err) => TestStatus::Failed {
                        reason: Some(format!("{err:#}")),
                    },
                }
            }
            PluginTestCase::BasicOutOfPlaceNoteProcessing => {
                // This test is very similar to `BasicAudioProcessing`, but it requires the
                // `note-ports` extension, sends notes and/or MIDI to the plugin, and doesn't
                // require the `audio-ports` extension
                let mut prng = new_prng();

                let host = ClapHost::new();
                let result = library
                    .create_plugin(plugin_id, host.clone())
                    .context("Could not create the plugin instance")
                    .and_then(|plugin| {
                        plugin.init().context("Error during initialization")?;

                        // You can have note/MIDI-only plugins, so not having any audio ports is
                        // perfectly fine here
                        let audio_ports_config = match plugin.get_extension::<AudioPorts>() {
                            Some(audio_ports) => audio_ports
                                .config()
                                .context("Error while querying 'audio-ports' IO configuration")?,
                            None => AudioPortConfig::default(),
                        };
                        let note_port_config =
                            match plugin.get_extension::<NotePorts>() {
                                Some(note_ports) => note_ports.config().context(
                                    "Error while querying 'note-ports' IO configuration",
                                )?,
                                None => return Ok(TestStatus::Skipped {
                                    reason: Some(String::from(
                                        "The plugin does not implement the 'note-ports' extension.",
                                    )),
                                }),
                            };

                        const BUFFER_SIZE: usize = 512;
                        let process_config = ProcessConfig {
                            sample_rate: 44_100.0,
                            tempo: 110.0,
                            time_sig_numerator: 4,
                            time_sig_denominator: 4,
                        };
                        let (mut input_buffers, mut output_buffers) =
                            audio_ports_config.create_buffers(BUFFER_SIZE);
                        let audio_buffers = AudioBuffers::OutOfPlace(
                            OutOfPlaceAudioBuffers::new(&mut input_buffers, &mut output_buffers)
                                .unwrap(),
                        );

                        // We'll fill the input event queue with (consistent) random CLAP note
                        // and/or MIDI events depending on what's supported by the plugin
                        // supports
                        let mut note_event_rng = NoteGenerator::new(note_port_config);

                        run_out_of_place_audio_processing_test(
                            &plugin,
                            audio_buffers,
                            process_config,
                            |process_data| {
                                note_event_rng.fill_event_queue(
                                    &mut prng,
                                    &process_data.input_events,
                                    BUFFER_SIZE as u32,
                                )?;
                                process_data.buffers.randomize(&mut prng);

                                Ok(())
                            },
                        )?;

                        host.thread_safety_check()
                            .context("Thread safety checks failed")?;

                        Ok(TestStatus::Success { notes: None })
                    });

                match result {
                    Ok(status) => status,
                    Err(err) => TestStatus::Failed {
                        reason: Some(format!("{err:#}")),
                    },
                }
            }
            PluginTestCase::InconsistentNoteProcessing => {
                // This is the same test as `BasicOutOfPlaceNoteProcessing`, but without
                // requiring matched note on/off pairs and similar invariants
                let mut prng = new_prng();

                let host = ClapHost::new();
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
                        let note_port_config =
                            match plugin.get_extension::<NotePorts>() {
                                Some(note_ports) => note_ports.config().context(
                                    "Error while querying 'note-ports' IO configuration",
                                )?,
                                None => return Ok(TestStatus::Skipped {
                                    reason: Some(String::from(
                                        "The plugin does not implement the 'note-ports' extension.",
                                    )),
                                }),
                            };

                        const BUFFER_SIZE: usize = 512;
                        let process_config = ProcessConfig {
                            sample_rate: 44_100.0,
                            tempo: 110.0,
                            time_sig_numerator: 4,
                            time_sig_denominator: 4,
                        };
                        let (mut input_buffers, mut output_buffers) =
                            audio_ports_config.create_buffers(BUFFER_SIZE);
                        // TODO: Use in-place processing for this test
                        let audio_buffers = AudioBuffers::OutOfPlace(
                            OutOfPlaceAudioBuffers::new(&mut input_buffers, &mut output_buffers)
                                .unwrap(),
                        );

                        // This RNG (Random Note Generator) allows generates mismatching events
                        let mut note_event_rng =
                            NoteGenerator::new(note_port_config).with_inconsistent_events();

                        run_out_of_place_audio_processing_test(
                            &plugin,
                            audio_buffers,
                            process_config,
                            |process_data| {
                                note_event_rng.fill_event_queue(
                                    &mut prng,
                                    &process_data.input_events,
                                    BUFFER_SIZE as u32,
                                )?;
                                process_data.buffers.randomize(&mut prng);

                                Ok(())
                            },
                        )?;

                        host.thread_safety_check()
                            .context("Thread safety checks failed")?;

                        Ok(TestStatus::Success { notes: None })
                    });

                match result {
                    Ok(status) => status,
                    Err(err) => TestStatus::Failed {
                        reason: Some(format!("{err:#}")),
                    },
                }
            }
        };

        self.create_result(status)
    }
}

/// Run the standard audio processing test for a **deactivated** plugin. This calls the process
/// function five times, and checks the output for consistency each time.
///
/// The `Preprocess` closure is called before each processing cycle to allow the process data to be
/// modified for the next process cycle.
fn run_out_of_place_audio_processing_test<Preprocess>(
    plugin: &Plugin,
    audio_buffers: AudioBuffers,
    process_config: ProcessConfig,
    mut preprocess: Preprocess,
) -> Result<()>
where
    Preprocess: FnMut(&mut ProcessData) -> Result<()> + Send,
{
    let buffer_size = audio_buffers.len();
    let mut process_data = ProcessData::new(audio_buffers, process_config);

    plugin.activate(process_config.sample_rate, 1, buffer_size)?;

    plugin.on_audio_thread(|plugin| -> Result<()> {
        plugin.start_processing()?;

        // This test is repeated a couple times
        // NOTE: We intentionally do not disable denormals here
        for iteration in 0..5 {
            preprocess(&mut process_data)?;

            // We'll check that the plugin hasn't modified the input buffers after the
            // test
            let original_input_buffers = process_data.buffers.inputs_ref().to_owned();

            plugin
                .process(&mut process_data)
                .context("Error during audio processing")?;
            check_out_of_place_output_consistency(&process_data, &original_input_buffers)
                .with_context(|| {
                    format!("Failed during processing cycle {} out of 5", iteration + 1)
                })?;

            process_data.clear_events();
            process_data.advance_transport(buffer_size as u32);
        }

        plugin.stop_processing();

        Ok(())
    })?;

    plugin.deactivate();

    Ok(())
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
                    anyhow::bail!("The sample written to output port {port_idx}, channel {channel_idx}, and sample index {sample_idx} is {sample:?}");
                } else if sample.is_subnormal() {
                    anyhow::bail!("The sample written to output port {port_idx}, channel {channel_idx}, and sample index {sample_idx} is subnormal ({sample:?})");
                }
            }
        }
    }

    // If the plugin output any events, then they should be in a monotonically increasing order
    let mut last_event_time = 0;
    #[allow(clippy::significant_drop_in_scrutinee)]
    for event in process_data.output_events.events.lock().unwrap().iter() {
        let event_time = event.header().time;
        if event_time < last_event_time {
            anyhow::bail!("The plugin output an event for sample {event_time} after it had previously output an event for sample {last_event_time}")
        }

        last_event_time = event_time;
    }

    Ok(())
}
