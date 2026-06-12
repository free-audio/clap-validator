//! Tests surrounding state handling.

use super::PluginInstanceTestCase;
use crate::plugin::ext::params::Params;
use crate::plugin::ext::state::State;
use crate::plugin::library::PluginLibrary;
use crate::plugin::process::{InputEventQueue, OutputEventQueue};
use crate::tests::plugin_instance::params::{param_generate_diff, param_get_values};
use crate::tests::rng::{ParamFuzzer, new_prng};
use crate::tests::{TestStatus, temporary_file};
use anyhow::{Context, Result};
use rand::RngExt;
use std::io::Write;

/// The file name we'll use to dump the expected state when a test fails.
const EXPECTED_STATE_FILE_NAME: &str = "state-expected";
/// The file name we'll use to dump the actual state when a test fails.
const ACTUAL_STATE_FILE_NAME: &str = "state-actual";

/// The test for `PluginTestCase::StateInvalidEmpty`.
pub fn test_state_invalid_empty(library: &PluginLibrary, plugin_id: &str) -> Result<TestStatus> {
    let plugin = library
        .create_plugin(plugin_id)
        .context("Could not create the plugin instance")?;

    plugin.init().context("Error during initialization")?;
    let state = match plugin.get_extension::<State>() {
        Some(state) => state,
        None => {
            return Ok(TestStatus::Skipped {
                details: Some(String::from("The plugin does not implement the 'state' extension.")),
            });
        }
    };

    let result = state.load(&[]);

    plugin.poll_callback(|_| Ok(()))?;

    match result {
        Ok(_) => Ok(TestStatus::Warning {
            details: Some(String::from(
                "The plugin returned true when 'clap_plugin_state::load()' was called when an empty state, this is \
                 likely a bug.",
            )),
        }),
        Err(_) => Ok(TestStatus::Success { details: None }),
    }
}

/// The test for `PluginTestCase::StateInvalidRandom`.
pub fn test_state_invalid_random(library: &PluginLibrary, plugin_id: &str) -> Result<TestStatus> {
    let mut prng = new_prng();

    let plugin = library
        .create_plugin(plugin_id)
        .context("Could not create the plugin instance")?;

    plugin.init().context("Error during initialization")?;

    let state = match plugin.get_extension::<State>() {
        Some(state) => state,
        None => {
            return Ok(TestStatus::Skipped {
                details: Some(String::from("The plugin does not implement the 'state' extension.")),
            });
        }
    };

    plugin.poll_callback(|_| Ok(()))?;

    let mut random_data = vec![0u8; 1024 * 1024];
    let mut succeeded = false;

    for _ in 0..3 {
        prng.fill(&mut random_data[..]);
        succeeded |= state.load(&random_data).is_ok();
    }

    plugin.poll_callback(|_| Ok(()))?;

    match succeeded {
        false => Ok(TestStatus::Success { details: None }),
        true => Ok(TestStatus::Warning {
            details: Some(String::from(
                "The plugin loaded random bytes successfully, which is unexpected, but the plugin did not crash.",
            )),
        }),
    }
}

/// The test for `PluginTestCase::StateReproducibilityNullCookies` and `PluginTestCase::StateReproducibilityBasic`.
/// See the description of these test for a detailed explanation, but we essentially check if saving a loaded state results in the
/// same state file, and whether a plugin's parameters are the same after loading the state.
///
/// The `zero_out_cookies` parameter offers an alternative on this test that sends parameter change
/// events with all cookies set to null pointers. The plugin should behave identically when this
/// happens.
pub fn test_state_reproducibility(
    library: &PluginLibrary,
    plugin_id: &str,
    buffered_streams: bool,
    binary_equality: bool,
) -> Result<TestStatus> {
    let mut prng = new_prng();

    let plugin = library
        .create_plugin(plugin_id)
        .context("Could not create the plugin instance")?;

    // We'll drop and reinitialize the plugin later
    let (expected_state, expected_param_values) = {
        plugin.init().context("Error during initialization")?;

        let params = match plugin.get_extension::<Params>() {
            Some(params) => params,
            None => {
                return Ok(TestStatus::Skipped {
                    details: Some(String::from("The plugin does not implement the 'params' extension.")),
                });
            }
        };

        let state = match plugin.get_extension::<State>() {
            Some(state) => state,
            None => {
                return Ok(TestStatus::Skipped {
                    details: Some(String::from("The plugin does not implement the 'state' extension.")),
                });
            }
        };

        plugin.poll_callback(|_| Ok(()))?;

        let param_info = params
            .info()
            .context("Failure while fetching the plugin's parameters")?;

        // We can't compare the values from these events direclty as the plugin
        // may round the values during the parameter set
        let param_fuzzer = ParamFuzzer::new(&param_info);
        let param_events: Vec<_> = param_fuzzer.randomize_params_at(&mut prng, 0).collect();

        {
            let input_queue = InputEventQueue::new();
            let output_queue = OutputEventQueue::new();
            input_queue.add_events(param_events);
            params.flush(&input_queue, &output_queue);
        }

        plugin.poll_callback(|_| Ok(()))?;

        // We'll check that the plugin has these sames values after reloading the state. These
        // values are rounded to the tenth decimal to provide some leeway in the serialization and
        // deserialization process.
        let expected_param_values = param_get_values(&params)?;
        let expected_state = if buffered_streams {
            state.save_buffered(23)?
        } else {
            state.save()?
        };

        plugin.poll_callback(|_| Ok(()))?;

        (expected_state, expected_param_values)
    };

    // Now we'll recreate the plugin instance, load the state, and check whether the values are
    // consistent and whether saving the state again results in an idential state file. This ends up
    // being a bit of a lengthy test case because of this multiple initialization. Before
    // continueing, we'll make sure the first plugin instance no longer exists.
    drop(plugin);

    let plugin = library
        .create_plugin(plugin_id)
        .context("Could not create the plugin instance a second time")?;

    plugin
        .init()
        .context("Error while initializing the second plugin instance")?;

    let params = match plugin.get_extension::<Params>() {
        Some(params) => params,
        None => {
            // I sure hope that no plugin will ever hit this
            return Ok(TestStatus::Skipped {
                details: Some(String::from("The plugin does not implement the 'params' extension.")),
            });
        }
    };

    let state = match plugin.get_extension::<State>() {
        Some(state) => state,
        None => {
            return Ok(TestStatus::Skipped {
                details: Some(String::from(
                    "The plugin's second instance does not implement the 'state' extension.",
                )),
            });
        }
    };

    plugin.poll_callback(|_| Ok(()))?;

    if buffered_streams {
        // This is a buffered load that only loads 17 bytes at a time. Why 17? Because.
        state.load_buffered(&expected_state, 17)?;
    } else {
        state.load(&expected_state)?;
    }

    plugin.poll_callback(|_| Ok(()))?;

    let actual_param_values = param_get_values(&params)?;

    if let Some(diff) = param_generate_diff(&actual_param_values, &expected_param_values, &params)? {
        anyhow::bail!(
            "After reloading the state, these parameter values do not match the old values: \n{}",
            diff
        );
    }

    plugin.poll_callback(|_| Ok(()))?;

    // Now for the moment of truth
    let actual_state = state.save()?;

    plugin.poll_callback(|_| Ok(()))?;

    if !binary_equality || actual_state == expected_state {
        Ok(TestStatus::Success { details: None })
    } else {
        let (expected_state_file_path, mut expected_state_file) = temporary_file(
            &PluginInstanceTestCase::StateReproducibilityBinary.to_string(),
            plugin_id,
            EXPECTED_STATE_FILE_NAME,
        )?;

        let (actual_state_file_path, mut actual_state_file) = temporary_file(
            &PluginInstanceTestCase::StateReproducibilityBinary.to_string(),
            plugin_id,
            ACTUAL_STATE_FILE_NAME,
        )?;

        expected_state_file.write_all(&expected_state)?;
        actual_state_file.write_all(&actual_state)?;

        Ok(TestStatus::Failed {
            details: Some(format!(
                "The saved state after loading differs from the original saved state. \nExpected: '{}'. \nActual: \
                 '{}'.",
                expected_state_file_path.display(),
                actual_state_file_path.display(),
            )),
        })
    }
}
