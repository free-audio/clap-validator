//! Tests surrounding state handling.

use anyhow::{Context, Result};
use clap_sys::id::clap_id;
use std::collections::BTreeMap;

use crate::hosting::ClapHost;
use crate::plugin::audio_thread::process::ProcessConfig;
use crate::plugin::ext::audio_ports::{AudioPortConfig, AudioPorts};
use crate::plugin::ext::params::Params;
use crate::plugin::ext::state::State;
use crate::plugin::library::PluginLibrary;
use crate::tests::rng::{new_prng, ParamFuzzer};
use crate::tests::TestStatus;

use super::processing::ProcessingTest;

/// The test for `PluginTestCase::BasicStateReproducibility`.
pub fn test_basic_state_reproducibility(library: &PluginLibrary, plugin_id: &str) -> TestStatus {
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
                    Some(audio_ports) => audio_ports
                        .config()
                        .context("Error while querying 'audio-ports' IO configuration")?,
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
                ProcessingTest::new_out_of_place(&plugin, &mut input_buffers, &mut output_buffers)?
                    .run_once(ProcessConfig::default(), move |process_data| {
                        *process_data.input_events.events.lock().unwrap() = random_param_set_events;

                        Ok(())
                    })?;

                // We'll check that the plugin has these sames values after reloading
                // the state. These values are rounded to the tenth decimal to provide
                // some leeway in the serialization and deserializatoin process.
                let expected_param_values: BTreeMap<clap_id, f64> = param_infos
                    .iter()
                    .map(|(param_id, _)| params.get(*param_id).map(|value| (*param_id, value)))
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
                            "The plugin's second instance does not support the 'params' extension.",
                        )),
                    });
                }
            };
            let state = match plugin.get_extension::<State>() {
                Some(state) => state,
                None => {
                    return Ok(TestStatus::Skipped {
                        reason: Some(String::from(
                            "The plugin's second instance does not support the 'state' extension.",
                        )),
                    })
                }
            };

            state.load(&state_file)?;
            let actual_param_values: BTreeMap<clap_id, f64> = expected_param_values
                .iter()
                .map(|(param_id, _)| params.get(*param_id).map(|value| (*param_id, value)))
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
                    "After reloading the state, the plugin's parameter values do not match the \
                     old values when queried through 'clap_plugin_params::get()'. The mismatching \
                     values are {incorrect_values}."
                );
            }

            // Now for the monent of truth
            let second_state_file = state.save()?;
            if second_state_file == state_file {
                Ok(TestStatus::Success { notes: None })
            } else {
                Ok(TestStatus::Failed {
                    reason: Some(String::from(
                        "Re-saving the loaded state resulted in a different state file.",
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
