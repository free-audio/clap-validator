//! Tests for individual plugin instances.

use anyhow::{Context, Result};
use clap::ValueEnum;
use clap_sys::id::clap_id;
use std::collections::BTreeMap;
use std::process::Command;

use self::processing::ProcessingTest;
use super::rng::ParamFuzzer;
use super::{TestCase, TestResult, TestStatus};
use crate::hosting::ClapHost;
use crate::plugin::audio_thread::process::ProcessConfig;
use crate::plugin::ext::audio_ports::{AudioPortConfig, AudioPorts};
use crate::plugin::ext::params::Params;
use crate::plugin::ext::state::State;
use crate::plugin::library::PluginLibrary;
use crate::tests::rng::new_prng;

mod params;
mod processing;

const BASIC_OUT_OF_PLACE_AUDIO_PROCESSING: &str = "process-audio-out-of-place-basic";
const BASIC_OUT_OF_PLACE_NOTE_PROCESSING: &str = "process-note-out-of-place-basic";
const INCONSISTENT_NOTE_PROCESSING: &str = "process-note-inconsistent";
const CONVERT_PARAMS: &str = "param-conversions";
const BASIC_STATE_REPRODUCIBILITY: &str = "state-reproducibility-basic";
const FLUSH_STATE_REPRODUCIBILITY: &str = "state-reproducibility-flush";

/// The tests for individual CLAP plugins. See the module's heading for more information, and the
/// `description` function below for a description of each test case.
pub enum PluginTestCase {
    BasicOutOfPlaceAudioProcessing,
    BasicOutOfPlaceNoteProcessing,
    InconsistentNoteProcessing,
    ConvertParams,
    BasicStateReproducibility,
    FlushStateReproducibility,
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
        PluginTestCase::BasicStateReproducibility,
        PluginTestCase::FlushStateReproducibility,
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
            BASIC_STATE_REPRODUCIBILITY => Some(PluginTestCase::BasicStateReproducibility),
            FLUSH_STATE_REPRODUCIBILITY => Some(PluginTestCase::FlushStateReproducibility),
            _ => None,
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            PluginTestCase::BasicOutOfPlaceAudioProcessing => BASIC_OUT_OF_PLACE_AUDIO_PROCESSING,
            PluginTestCase::BasicOutOfPlaceNoteProcessing => BASIC_OUT_OF_PLACE_NOTE_PROCESSING,
            PluginTestCase::InconsistentNoteProcessing => INCONSISTENT_NOTE_PROCESSING,
            PluginTestCase::ConvertParams => CONVERT_PARAMS,
            PluginTestCase::BasicStateReproducibility => BASIC_STATE_REPRODUCIBILITY,
            PluginTestCase::FlushStateReproducibility => FLUSH_STATE_REPRODUCIBILITY,
        }
    }

    fn description(&self) -> String {
        match self {
            PluginTestCase::BasicOutOfPlaceAudioProcessing => String::from(
                "Processes random audio through the plugin with its default parameter values and \
                 tests whether the output does not contain any non-finite or subnormal values. \
                 Uses out-of-place audio processing.",
            ),
            PluginTestCase::BasicOutOfPlaceNoteProcessing => String::from(
                "Sends audio and random note and MIDI events to the plugin with its default \
                 parameter values and tests the output for consistency. Uses out-of-place audio \
                 processing.",
            ),
            PluginTestCase::InconsistentNoteProcessing => String::from(
                "Sends intentionally inconsistent and mismatching note and MIDI events to the \
                 plugin with its default parameter values and tests the output for consistency. \
                 Uses out-of-place audio processing.",
            ),
            PluginTestCase::ConvertParams => String::from(
                "Asserts that value to string and string to value conversions are supported for \
                 ether all or none of the plugin's parameters, and that conversions between \
                 values and strings roundtrip consistently.",
            ),
            PluginTestCase::BasicStateReproducibility => String::from(
                "Randomizes a plugin's parameters, saves its state, recreates the plugin \
                 instance, reloads the state, and then checks whether the parameter values are \
                 the same and whether saving the state once more results in the same state file \
                 as before. The parameter values are updated using the process function.",
            ),
            PluginTestCase::FlushStateReproducibility => String::from(
                "Randomizes a plugin's parameters, saves its state, recreates the plugin \
                 instance, sets the same parameters as before, saves the state again, and then \
                 asserts that the two states are identical. The parameter values are set updated \
                 using the process function to create the first state, and using the flush \
                 function to create the second state.",
            ),
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
                processing::test_basic_out_of_place_audio_processing(library, plugin_id)
            }
            PluginTestCase::BasicOutOfPlaceNoteProcessing => {
                processing::test_basic_out_of_place_note_processing(library, plugin_id)
            }
            PluginTestCase::InconsistentNoteProcessing => {
                processing::test_inconsistent_note_processing(library, plugin_id)
            }
            PluginTestCase::ConvertParams => params::test_convert_params(library, plugin_id),
            PluginTestCase::BasicStateReproducibility => {
                // See the description of this test for a detailed explanation, but we essentially
                // check if saving a loaded state results in the same state file, and whether a
                // plugin's parameters are the same after loading the state.
                let mut prng = new_prng();

                let host = ClapHost::new();
                let result = library
                    .create_plugin(plugin_id, host.clone())
                    .context("Could not create the plugin instance")
                    .and_then(|plugin| {
                        // We'll drop and reinitialize the plugin later
                        let (state_file, expected_param_values) = {
                            plugin.init().context("Error during initialization")?;

                            let audio_ports_config = match plugin.get_extension::<AudioPorts>() {
                                Some(audio_ports) => audio_ports.config().context(
                                    "Error while querying 'audio-ports' IO configuration",
                                )?,
                                None => AudioPortConfig::default(),
                            };
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
                            let state = match plugin.get_extension::<State>() {
                                Some(state) => state,
                                None => {
                                    return Ok(TestStatus::Skipped {
                                        reason: Some(String::from(
                                            "The plugin does not support the 'state' extension.",
                                        )),
                                    })
                                }
                            };

                            let param_infos = params
                                .info()
                                .context("Failure while fetching the plugin's parameters")?;

                            // We can't compare the values from these events direclty as the plugin
                            // may round the values during the parameter set
                            let param_fuzzer = ParamFuzzer::new(&param_infos);
                            let random_param_set_events: Vec<_> =
                                param_fuzzer.randomize_params_at(&mut prng, 0).collect();

                            let (mut input_buffers, mut output_buffers) =
                                audio_ports_config.create_buffers(512);
                            ProcessingTest::new_out_of_place(
                                &plugin,
                                &mut input_buffers,
                                &mut output_buffers,
                            )?
                            .run_once(
                                ProcessConfig::default(),
                                move |process_data| {
                                    *process_data.input_events.events.lock().unwrap() =
                                        random_param_set_events;

                                    Ok(())
                                },
                            )?;

                            // We'll check that the plugin has these sames values after reloading
                            // the state. These values are rounded to the tenth decimal to provide
                            // some leeway in the serialization and deserializatoin process.
                            let expected_param_values: BTreeMap<clap_id, f64> = param_infos
                                .iter()
                                .map(|(param_id, _)| {
                                    params.get(*param_id).map(|value| (*param_id, value))
                                })
                                .collect::<Result<BTreeMap<clap_id, f64>>>()?;

                            let state_file = state.save()?;

                            (state_file, expected_param_values)
                        };

                        // Now we'll recreate the plugin instance, load the state, and check whether
                        // the values are consistent and whether saving the state again results in
                        // an idential state file. This ends up being a bit of a lengthy test case
                        // because of this multiple initialization. Before continueing, we'll make
                        // sure the first plugin instance no longer exists.
                        drop(plugin);

                        let plugin = library
                            .create_plugin(plugin_id, host.clone())
                            .context("Could not create the plugin instance a second time")?;
                        plugin
                            .init()
                            .context("Error while initializing the second plugin instance")?;

                        let params = match plugin.get_extension::<Params>() {
                            Some(params) => params,
                            None => {
                                // I sure hope that no plugin will eer hit this
                                return Ok(TestStatus::Skipped {
                                    reason: Some(String::from(
                                        "The plugin's second instance does not support the \
                                         'params' extension.",
                                    )),
                                });
                            }
                        };
                        let state = match plugin.get_extension::<State>() {
                            Some(state) => state,
                            None => {
                                return Ok(TestStatus::Skipped {
                                    reason: Some(String::from(
                                        "The plugin's second instance does not support the \
                                         'state' extension.",
                                    )),
                                })
                            }
                        };

                        state.load(&state_file)?;
                        let actual_param_values: BTreeMap<clap_id, f64> = expected_param_values
                            .iter()
                            .map(|(param_id, _)| {
                                params.get(*param_id).map(|value| (*param_id, value))
                            })
                            .collect::<Result<BTreeMap<clap_id, f64>>>()?;
                        if actual_param_values != expected_param_values {
                            let param_infos = params
                                .info()
                                .context("Failure while fetching the plugin's parameters")?;

                            // To avoid flooding the output too much, we'll print only the different
                            // values
                            let incorrect_values: String = actual_param_values
                                .into_iter()
                                .filter_map(|(param_id, actual_value)| {
                                    let expected_value = expected_param_values[&param_id];
                                    if actual_value == expected_value {
                                        None
                                    } else {
                                        let param_name = &param_infos[&param_id].name;
                                        Some(format!(
                                            "parameter {param_id} ('{param_name}'), expected \
                                             {expected_value:?}, actual {actual_value:?}"
                                        ))
                                    }
                                })
                                .collect::<Vec<String>>()
                                .join(", ");

                            anyhow::bail!(
                                "After reloading the state, the plugin's parameter values do not \
                                 match the old values when queried through \
                                 'clap_plugin_params::get()'. The mismatching values are \
                                 {incorrect_values}."
                            );
                        }

                        // Now for the monent of truth
                        let second_state_file = state.save()?;
                        if second_state_file == state_file {
                            Ok(TestStatus::Success { notes: None })
                        } else {
                            Ok(TestStatus::Failed {
                                reason: Some(String::from(
                                    "Re-saving the loaded state resulted in a different state \
                                     file.",
                                )),
                            })
                        }
                    });

                match result {
                    Ok(status) => status,
                    Err(err) => TestStatus::Failed {
                        reason: Some(format!("{err:#}")),
                    },
                }
            }
            PluginTestCase::FlushStateReproducibility => TestStatus::Skipped {
                reason: Some(String::from("Not yet implemented")),
            },
        };

        self.create_result(status)
    }
}
