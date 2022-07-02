//! Tests for individual plugin instances.

use anyhow::Context;
use clap::ValueEnum;
use rand::Rng;
use std::process::Command;

use self::processing::ProcessingTest;

use super::{TestCase, TestResult, TestStatus};
use crate::hosting::ClapHost;
use crate::plugin::audio_thread::process::ProcessConfig;
use crate::plugin::ext::audio_ports::{AudioPortConfig, AudioPorts};
use crate::plugin::ext::note_ports::NotePorts;
use crate::plugin::ext::params::Params;
use crate::plugin::library::PluginLibrary;
use crate::tests::rng::{new_prng, NoteGenerator};

mod processing;

const BASIC_OUT_OF_PLACE_AUDIO_PROCESSING: &str = "process-audio-out-of-place-basic";
const BASIC_OUT_OF_PLACE_NOTE_PROCESSING: &str = "process-note-out-of-place-basic";
const INCONSISTENT_NOTE_PROCESSING: &str = "process-note-inconsistent";
const CONVERT_PARAMS: &str = "param-conversions";

/// The tests for individual CLAP plugins. See the module's heading for more information, and the
/// `description` function below for a description of each test case.
pub enum PluginTestCase {
    BasicOutOfPlaceAudioProcessing,
    BasicOutOfPlaceNoteProcessing,
    InconsistentNoteProcessing,
    ConvertParams,
}

impl<'a> TestCase<'a> for PluginTestCase {
    /// A loaded CLAP plugin library and the ID of the plugin contained within that library that
    /// should be tested.
    type TestArgs = (&'a PluginLibrary, &'a str);

    const ALL: &'static [Self] = &[
        PluginTestCase::BasicOutOfPlaceAudioProcessing,
        PluginTestCase::BasicOutOfPlaceNoteProcessing,
        PluginTestCase::InconsistentNoteProcessing,
        PluginTestCase::ConvertParams,
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
            CONVERT_PARAMS => Some(PluginTestCase::ConvertParams),
            _ => None,
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            PluginTestCase::BasicOutOfPlaceAudioProcessing => BASIC_OUT_OF_PLACE_AUDIO_PROCESSING,
            PluginTestCase::BasicOutOfPlaceNoteProcessing => BASIC_OUT_OF_PLACE_NOTE_PROCESSING,
            PluginTestCase::InconsistentNoteProcessing => INCONSISTENT_NOTE_PROCESSING,
            PluginTestCase::ConvertParams => CONVERT_PARAMS,
        }
    }

    fn description(&self) -> String {
        match self {
            PluginTestCase::BasicOutOfPlaceAudioProcessing => String::from("Processes random audio through the plugin with its default parameter values and tests whether the output does not contain any non-finite or subnormal values. Uses out-of-place audio processing."),
            PluginTestCase::BasicOutOfPlaceNoteProcessing => String::from("Sends audio and random note and MIDI events to the plugin with its default parameter values and tests the output for consistency. Uses out-of-place audio processing."),
            PluginTestCase::InconsistentNoteProcessing => String::from("Sends intentionally inconsistent and mismatching note and MIDI events to the plugin with its default parameter values and tests the output for consistency. Uses out-of-place audio processing."),
            PluginTestCase::ConvertParams => String::from("Asserts that value to string and string to value conversions are supported for ether all or none of the plugin's parameters, and that conversions between values and strings roundtrip consistently."),
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

                        let (mut input_buffers, mut output_buffers) =
                            audio_ports_config.create_buffers(512);
                        ProcessingTest::new_out_of_place(
                            &plugin,
                            &mut input_buffers,
                            &mut output_buffers,
                        )?
                        .run(
                            ProcessConfig {
                                sample_rate: 44_100.0,
                                tempo: 110.0,
                                time_sig_numerator: 4,
                                time_sig_denominator: 4,
                            },
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
                        if note_port_config.inputs.is_empty() {
                            return Ok(TestStatus::Skipped {
                                reason: Some(String::from(
                                    "The plugin implements the 'note-ports' extension but it does not have any input note ports.",
                                )),
                            });
                        }

                        // We'll fill the input event queue with (consistent) random CLAP note
                        // and/or MIDI events depending on what's supported by the plugin
                        // supports
                        let mut note_event_rng = NoteGenerator::new(note_port_config);

                        const BUFFER_SIZE: usize = 512;
                        let (mut input_buffers, mut output_buffers) =
                            audio_ports_config.create_buffers(512);
                        ProcessingTest::new_out_of_place(
                            &plugin,
                            &mut input_buffers,
                            &mut output_buffers,
                        )?
                        .run(
                            ProcessConfig {
                                sample_rate: 44_100.0,
                                tempo: 110.0,
                                time_sig_numerator: 4,
                                time_sig_denominator: 4,
                            },
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
                        if note_port_config.inputs.is_empty() {
                            return Ok(TestStatus::Skipped {
                                reason: Some(String::from(
                                    "The plugin implements the 'note-ports' extension but it does not have any input note ports.",
                                )),
                            });
                        }

                        // This RNG (Random Note Generator) allows generates mismatching events
                        let mut note_event_rng =
                            NoteGenerator::new(note_port_config).with_inconsistent_events();

                        // TODO: Use in-place processing for this test
                        const BUFFER_SIZE: usize = 512;
                        let (mut input_buffers, mut output_buffers) =
                            audio_ports_config.create_buffers(512);
                        ProcessingTest::new_out_of_place(
                            &plugin,
                            &mut input_buffers,
                            &mut output_buffers,
                        )?
                        .run(
                            ProcessConfig {
                                sample_rate: 44_100.0,
                                tempo: 110.0,
                                time_sig_numerator: 4,
                                time_sig_denominator: 4,
                            },
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
            PluginTestCase::ConvertParams => {
                let mut prng = new_prng();

                let host = ClapHost::new();
                let result = library
                    .create_plugin(plugin_id, host.clone())
                    .context("Could not create the plugin instance")
                    .and_then(|plugin| {
                        plugin.init().context("Error during initialization")?;

                        let params = match plugin.get_extension::<Params>() {
                            Some(params) => params,
                            None => {
                                return Ok(TestStatus::Skipped {
                                    reason: Some(String::from(
                                        "The plugin does not support the 'params' extension.",
                                    )),
                                })
                            }
                        };

                        let param_infos = params
                            .info()
                            .context("Failure while fetching the plugin's parameters")?;
                        let num_params = param_infos.len();

                        // We keep track of how many parameters support these conversions. A plugin
                        // should support either conversion either for all of its parameters, or for
                        // none of them.
                        const VALUES_PER_PARAM: usize = 6;
                        let expected_conversions = param_infos.len() * VALUES_PER_PARAM;

                        let mut num_supported_value_to_text = 0;
                        let mut num_supported_text_to_value = 0;
                        'param_loop: for (param_id, param_info) in param_infos {
                            let param_name = &param_info.name;

                            // For each parameter we'll test this for the minimum and maximum values
                            // (in case these values have special meanings), and four other random
                            // values
                            let values: [f64; VALUES_PER_PARAM] = [
                                *param_info.range.start(),
                                *param_info.range.end(),
                                prng.gen_range(param_info.range.clone()),
                                prng.gen_range(param_info.range.clone()),
                                prng.gen_range(param_info.range.clone()),
                                prng.gen_range(param_info.range),
                            ];
                            for starting_value in values {
                                // If the plugin rounds string representations then `value` may very
                                // will not roundtrip correctly, so we'll start at the string
                                // representation
                                let starting_text =
                                    match params.value_to_text(param_id, starting_value) {
                                        Ok(text) => text,
                                        Err(_) => continue 'param_loop,
                                    };
                                num_supported_value_to_text += 1;
                                let reconverted_value =
                                    match params.text_to_value(param_id, &starting_text) {
                                        Ok(value) => value,
                                        Err(_) => continue 'param_loop,
                                    };
                                num_supported_text_to_value += 1;

                                let reconverted_text =
                                    params.value_to_text(param_id, reconverted_value)?;
                                // Both of these are produced by the plugin, so they should be equal
                                if starting_text != reconverted_text {
                                    anyhow::bail!("Converting {starting_value:?} to a string, back to a value, and then back to a string again for parameter {param_id} ('{param_name}') results in '{starting_text}' -> {reconverted_value:?} -> '{reconverted_text}', which is not consistent.");
                                }

                                // And one last hop back for good measure
                                let final_value =
                                    params.text_to_value(param_id, &reconverted_text)?;
                                if final_value != reconverted_value {
                                    anyhow::bail!("Converting {starting_value:?} to a string, back to a value, back to a string, and then back to a value again for parameter {param_id} ('{param_name}') results in '{starting_text}' -> {reconverted_value:?} -> '{reconverted_text}' -> {final_value:?}, which is not consistent.");
                                }
                            }
                        }

                        if !(num_supported_value_to_text == 0 || num_supported_value_to_text == expected_conversions) {
                            anyhow::bail!("'clap_plugin_params::value_to_text()' returned true for {num_supported_value_to_text} out of {expected_conversions} calls. This function is expected to be supported for either none of the parameters or for all of them.");
                        }
                        if !(num_supported_text_to_value == 0 || num_supported_text_to_value == expected_conversions) {
                            anyhow::bail!("'clap_plugin_params::text_to_value()' returned true for {num_supported_text_to_value} out of {expected_conversions} calls. This function is expected to be supported for either none of the parameters or for all of them.");
                        }

                        host.thread_safety_check()
                            .context("Thread safety checks failed")?;

                        if num_supported_value_to_text == 0 || num_supported_text_to_value == 0 {
                            Ok(TestStatus::Skipped { reason: Some(String::from("The plugin doesn't support value to text and/or text to value conversions for parameters.")) })
                        } else {
                            Ok(TestStatus::Success { notes: None })
                        }
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
