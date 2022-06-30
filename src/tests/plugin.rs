//! Tests for individual plugin instances.

use anyhow::{Context, Result};
use clap::ValueEnum;
use std::process::Command;

use super::{TestCase, TestResult, TestStatus};
use crate::hosting::ClapHost;
use crate::plugin::audio_thread::process::{AudioBuffers, OutOfPlaceAudioBuffers, ProcessData};
use crate::plugin::ext::audio_ports::{AudioPortConfig, AudioPorts};
use crate::plugin::ext::note_ports::NotePorts;
use crate::plugin::library::PluginLibrary;
use crate::tests::rng::new_prng;

const BASIC_OUT_OF_PLACE_AUDIO_PROCESSING: &str = "process-audio-out-of-place-basic";
const BASIC_OUT_OF_PLACE_MIDI_PROCESSING: &str = "process-midi-out-of-place-basic";

/// The tests for individual CLAP plugins. See the module's heading for more information, and the
/// `description` function below for a description of each test case.
pub enum PluginTestCase {
    BasicOutOfPlaceAudioProcessing,
    BasicOutOfPlaceMidiProcessing,
}

impl<'a> TestCase<'a> for PluginTestCase {
    /// A loaded CLAP plugin library and the ID of the plugin contained within that library that
    /// should be tested.
    type TestArgs = (&'a PluginLibrary, &'a str);

    const ALL: &'static [Self] = &[
        PluginTestCase::BasicOutOfPlaceAudioProcessing,
        PluginTestCase::BasicOutOfPlaceMidiProcessing,
    ];

    fn from_str(string: &str) -> Option<Self> {
        match string {
            BASIC_OUT_OF_PLACE_AUDIO_PROCESSING => {
                Some(PluginTestCase::BasicOutOfPlaceAudioProcessing)
            }
            BASIC_OUT_OF_PLACE_MIDI_PROCESSING => {
                Some(PluginTestCase::BasicOutOfPlaceMidiProcessing)
            }
            _ => None,
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            PluginTestCase::BasicOutOfPlaceAudioProcessing => BASIC_OUT_OF_PLACE_AUDIO_PROCESSING,
            PluginTestCase::BasicOutOfPlaceMidiProcessing => BASIC_OUT_OF_PLACE_MIDI_PROCESSING,
        }
    }

    fn description(&self) -> String {
        match self {
            PluginTestCase::BasicOutOfPlaceAudioProcessing => String::from("Processes random audio through the plugin with its default parameter values and tests whether the output does not contain any non-finite or subnormal values. Uses out-of-place audio processing."),
            PluginTestCase::BasicOutOfPlaceMidiProcessing => String::from("Sends audio and random note and MIDI events to the plugin with its default parameter values and tests whether the output does not contain any non-finite or subnormal values. Uses out-of-place audio processing."),
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
        let status =
            match self {
                PluginTestCase::BasicOutOfPlaceAudioProcessing => {
                    let mut prng = new_prng();

                    // The host doesn't need to do anything special for this test
                    let host = ClapHost::new();
                    let result = library
                        .create_plugin(plugin_id, host.clone())
                        .context("Could not create the plugin instance")
                        .and_then(|plugin| {
                            plugin.init().context("Error during initialization")?;

                            let audio_ports_config = match plugin.get_extension::<AudioPorts>() {
                                Some(audio_ports) => audio_ports.config().context(
                                    "Error while querying 'audio-ports' IO configuration",
                                )?,
                                None => return Ok(TestStatus::Skipped {
                                    reason: Some(String::from(
                                        "The plugin does not support the 'audio-ports' extension.",
                                    )),
                                }),
                            };

                            const SAMPLE_RATE: f64 = 44_100.0;
                            const BUFFER_SIZE: usize = 512;
                            const TEMPO: f64 = 110.0;
                            const TIME_SIG_NUMERATOR: u16 = 4;
                            const TIME_SIG_DENOMINATOR: u16 = 4;

                            // This test only uses out-of-place processing
                            let (mut input_buffers, mut output_buffers) =
                                audio_ports_config.create_buffers(BUFFER_SIZE);
                            let mut process_data = ProcessData::new(
                                AudioBuffers::OutOfPlace(
                                    OutOfPlaceAudioBuffers::new(
                                        &mut input_buffers,
                                        &mut output_buffers,
                                    )
                                    .unwrap(),
                                ),
                                SAMPLE_RATE,
                                TEMPO,
                                TIME_SIG_NUMERATOR,
                                TIME_SIG_DENOMINATOR,
                            );

                            plugin.activate(SAMPLE_RATE, 1, BUFFER_SIZE)?;

                            plugin.on_audio_thread(|plugin| -> Result<()> {
                                plugin.start_processing()?;

                                // This test is repeated a couple times
                                // NOTE: We intentionally do not disable denormals here
                                for iteration in 0..5 {
                                    // We'll check that the plugin hasn't modified the input buffers after the
                                    // test
                                    process_data.buffers.randomize(&mut prng);
                                    let original_input_buffers =
                                        process_data.buffers.inputs_ref().to_owned();

                                    plugin
                                        .process(&mut process_data)
                                        .context("Error during audio processing")?;

                                    check_buffer_consistency(
                                        process_data.buffers.inputs_ref(),
                                        &original_input_buffers,
                                        process_data.buffers.outputs_ref(),
                                    )
                                    .with_context(|| {
                                        format!(
                                            "Failed during processing cycle {} out of 5",
                                            iteration + 1
                                        )
                                    })?;
                                }

                                plugin.stop_processing();

                                Ok(())
                            })?;

                            plugin.deactivate();

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
                PluginTestCase::BasicOutOfPlaceMidiProcessing => {
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
                                Some(audio_ports) => audio_ports.config().context(
                                    "Error while querying 'audio-ports' IO configuration",
                                )?,
                                None => AudioPortConfig::default(),
                            };
                            let note_port_config = match plugin.get_extension::<NotePorts>() {
                                Some(note_ports) => note_ports.config().context(
                                    "Error while querying 'note-ports' IO configuration",
                                )?,
                                None => return Ok(TestStatus::Skipped {
                                    reason: Some(String::from(
                                        "The plugin does not implement the 'note-ports' extension.",
                                    )),
                                }),
                            };

                            const SAMPLE_RATE: f64 = 44_100.0;
                            const BUFFER_SIZE: usize = 512;
                            const TEMPO: f64 = 110.0;
                            const TIME_SIG_NUMERATOR: u16 = 4;
                            const TIME_SIG_DENOMINATOR: u16 = 4;

                            // This test only uses out-of-place processing
                            let (mut input_buffers, mut output_buffers) =
                                audio_ports_config.create_buffers(BUFFER_SIZE);
                            let mut process_data = ProcessData::new(
                                AudioBuffers::OutOfPlace(
                                    OutOfPlaceAudioBuffers::new(
                                        &mut input_buffers,
                                        &mut output_buffers,
                                    )
                                    .unwrap(),
                                ),
                                SAMPLE_RATE,
                                TEMPO,
                                TIME_SIG_NUMERATOR,
                                TIME_SIG_DENOMINATOR,
                            );

                            // TODO: Send random notes and/or MIDI to the plugin
                            log::debug!("TODO: This test does not yet generate random notes/MIDI");

                            plugin.activate(SAMPLE_RATE, 1, BUFFER_SIZE)?;
                            plugin.on_audio_thread(|plugin| -> Result<()> {
                                plugin.start_processing()?;
                                for iteration in 0..5 {
                                    process_data.buffers.randomize(&mut prng);
                                    let original_input_buffers =
                                        process_data.buffers.inputs_ref().to_owned();

                                    plugin
                                        .process(&mut process_data)
                                        .context("Error during audio processing")?;
                                    check_buffer_consistency(
                                        process_data.buffers.inputs_ref(),
                                        &original_input_buffers,
                                        process_data.buffers.outputs_ref(),
                                    )
                                    .with_context(|| {
                                        format!(
                                            "Failed during processing cycle {} out of 5",
                                            iteration + 1
                                        )
                                    })?;
                                }
                                plugin.stop_processing();

                                Ok(())
                            })?;
                            plugin.deactivate();

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

/// Check whether the output buffer doesn't contain any NaN, infinite, or denormal values, and that
/// the input buffers have not been modified by the plugin.
fn check_buffer_consistency(
    input_buffers: &[Vec<Vec<f32>>],
    original_input_buffers: &[Vec<Vec<f32>>],
    output_buffers: &[Vec<Vec<f32>>],
) -> Result<()> {
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

    Ok(())
}
