//! Tests surrounding state handling.

use anyhow::{Context, Result};
use clap_sys::id::clap_id;
use std::collections::BTreeMap;
use std::io::Write;

use crate::host::Host;
use crate::plugin::ext::audio_ports::{AudioPortConfig, AudioPorts};
use crate::plugin::ext::params::{ParamInfo, Params};
use crate::plugin::ext::state::State;
use crate::plugin::instance::audio_thread::process::{Event, EventQueue, ProcessConfig};
use crate::plugin::library::PluginLibrary;
use crate::tests::rng::{new_prng, ParamFuzzer};
use crate::tests::{TestCase, TestStatus};

use super::processing::ProcessingTest;
use super::PluginTestCase;

/// The file name we'll use to dump the expected state when a test fails.
const EXPECTED_STATE_FILE_NAME: &str = "state-expected";
/// The file name we'll use to dump the actual state when a test fails.
const ACTUAL_STATE_FILE_NAME: &str = "state-actual";

/// The test for `PluginTestCase::InvalidState`.
pub fn test_invalid_state(library: &PluginLibrary, plugin_id: &str) -> Result<TestStatus> {
    let host = Host::new();
    let plugin = library
        .create_plugin(plugin_id, host.clone())
        .context("Could not create the plugin instance")?;

    plugin.init().context("Error during initialization")?;
    let state = match plugin.get_extension::<State>() {
        Some(state) => state,
        None => {
            return Ok(TestStatus::Skipped {
                details: Some(String::from(
                    "The plugin does not support the 'state' extension.",
                )),
            })
        }
    };
    host.handle_callbacks_once();

    match state.load(&[]) {
        Ok(_) => anyhow::bail!(
            "The plugin returned true when 'clap_plugin_state::load()' was called when an empty \
             state, this is likely a bug."
        ),
        Err(_) => {
            host.handle_callbacks_once();
            host.thread_safety_check()
                .context("Thread safety checks failed")?;

            Ok(TestStatus::Success { details: None })
        }
    }
}

/// The test for `PluginTestCase::BasicStateReproducibility`. See the description of this test for a
/// detailed explanation, but we essentially check if saving a loaded state results in the same
/// state file, and whether a plugin's parameters are the same after loading the state.
///
/// The `zero_out_cookies` parameter offers an alternative on this test that sends parameter change
/// events with all cookies set to null pointers. The plugin should behave identically when this
/// happens.
pub fn test_basic_state_reproducibility(
    library: &PluginLibrary,
    plugin_id: &str,
    zero_out_cookies: bool,
) -> Result<TestStatus> {
    let mut prng = new_prng();

    let host = Host::new();
    let plugin = library
        .create_plugin(plugin_id, host.clone())
        .context("Could not create the plugin instance")?;

    // We'll drop and reinitialize the plugin later
    let (expected_state, expected_param_values) = {
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
                    details: Some(String::from(
                        "The plugin does not support the 'params' extension.",
                    )),
                })
            }
        };
        let state = match plugin.get_extension::<State>() {
            Some(state) => state,
            None => {
                return Ok(TestStatus::Skipped {
                    details: Some(String::from(
                        "The plugin does not support the 'state' extension.",
                    )),
                })
            }
        };
        host.handle_callbacks_once();

        let param_infos = params
            .info()
            .context("Failure while fetching the plugin's parameters")?;

        // We can't compare the values from these events direclty as the plugin
        // may round the values during the parameter set
        let param_fuzzer = ParamFuzzer::new(&param_infos);
        let mut random_param_set_events: Vec<_> =
            param_fuzzer.randomize_params_at(&mut prng, 0).collect();

        // This is a variation on the test that checks whether the plugin handles null
        // pointer cookies correctly
        if zero_out_cookies {
            for event in &mut random_param_set_events {
                match event {
                    Event::ParamValue(event) => {
                        event.cookie = std::ptr::null_mut();
                    }
                    event => {
                        panic!("Unexpected event {event:?}, this is a clap-validator bug")
                    }
                }
            }
        }

        let (mut input_buffers, mut output_buffers) = audio_ports_config.create_buffers(512);
        ProcessingTest::new_out_of_place(&plugin, &mut input_buffers, &mut output_buffers)?
            .run_once(ProcessConfig::default(), move |process_data| {
                *process_data.input_events.events.lock() = random_param_set_events;

                Ok(())
            })?;

        // We'll check that the plugin has these sames values after reloading the state. These
        // values are rounded to the tenth decimal to provide some leeway in the serialization and
        // deserializatoin process.
        let expected_param_values: BTreeMap<clap_id, f64> = param_infos
            .keys()
            .map(|param_id| params.get(*param_id).map(|value| (*param_id, value)))
            .collect::<Result<BTreeMap<clap_id, f64>>>()?;

        let expected_state = state.save()?;
        host.handle_callbacks_once();

        (expected_state, expected_param_values)
    };

    // Now we'll recreate the plugin instance, load the state, and check whether the values are
    // consistent and whether saving the state again results in an idential state file. This ends up
    // being a bit of a lengthy test case because of this multiple initialization. Before
    // continueing, we'll make sure the first plugin instance no longer exists.
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
                details: Some(String::from(
                    "The plugin's second instance does not support the 'params' extension.",
                )),
            });
        }
    };
    let state = match plugin.get_extension::<State>() {
        Some(state) => state,
        None => {
            return Ok(TestStatus::Skipped {
                details: Some(String::from(
                    "The plugin's second instance does not support the 'state' extension.",
                )),
            })
        }
    };
    host.handle_callbacks_once();

    state.load(&expected_state)?;
    host.handle_callbacks_once();

    let actual_param_values: BTreeMap<clap_id, f64> = expected_param_values
        .keys()
        .map(|param_id| params.get(*param_id).map(|value| (*param_id, value)))
        .collect::<Result<BTreeMap<clap_id, f64>>>()?;
    if actual_param_values != expected_param_values {
        let param_infos = params
            .info()
            .context("Failure while fetching the plugin's parameters")?;

        // To avoid flooding the output too much, we'll print only the different values
        anyhow::bail!(
            "After reloading the state, the plugin's parameter values do not match the old values \
             when queried through 'clap_plugin_params::get()'. The mismatching values are {}.",
            format_mismatching_values(actual_param_values, &expected_param_values, &param_infos)
        );
    }

    // Now for the monent of truth
    let actual_state = state.save()?;
    host.handle_callbacks_once();

    host.thread_safety_check()
        .context("Thread safety checks failed")?;
    if actual_state == expected_state {
        Ok(TestStatus::Success { details: None })
    } else {
        let (expected_state_file_path, mut expected_state_file) =
            PluginTestCase::BasicStateReproducibility
                .temporary_file(plugin_id, EXPECTED_STATE_FILE_NAME)?;
        let (actual_state_file_path, mut actual_state_file) =
            PluginTestCase::BasicStateReproducibility
                .temporary_file(plugin_id, ACTUAL_STATE_FILE_NAME)?;

        expected_state_file.write_all(&expected_state)?;
        actual_state_file.write_all(&actual_state)?;

        anyhow::bail!(
            "Re-saving the loaded state resulted in a different state file. Expected: '{}'. \
             Actual: '{}'.",
            expected_state_file_path.display(),
            actual_state_file_path.display(),
        )
    }
}

/// The test for `PluginTestCase::FlushStateReproducibility`.
pub fn test_flush_state_reproducibility(
    library: &PluginLibrary,
    plugin_id: &str,
) -> Result<TestStatus> {
    let mut prng = new_prng();

    let host = Host::new();
    let plugin = library
        .create_plugin(plugin_id, host.clone())
        .context("Could not create the plugin instance")?;

    // We'll drop and reinitialize the plugin later. This first pass sets the values using the flush
    // function, and the second pass we'll compare this to uses the process function. We'll reuse
    // the parameter set events, but the cookies need to be updated first or they'll point to old
    // data.
    let (expected_state, old_random_param_set_events, expected_param_values) = {
        plugin.init().context("Error during initialization")?;

        let params = match plugin.get_extension::<Params>() {
            Some(params) => params,
            None => {
                return Ok(TestStatus::Skipped {
                    details: Some(String::from(
                        "The plugin does not support the 'params' extension.",
                    )),
                })
            }
        };
        let state = match plugin.get_extension::<State>() {
            Some(state) => state,
            None => {
                return Ok(TestStatus::Skipped {
                    details: Some(String::from(
                        "The plugin does not support the 'state' extension.",
                    )),
                })
            }
        };
        host.handle_callbacks_once();

        let param_infos = params
            .info()
            .context("Failure while fetching the plugin's parameters")?;

        // Make sure the flush does _something_. If nothing changes, then the plugin has not
        // implemented flush.
        let initial_param_values: BTreeMap<clap_id, f64> = param_infos
            .keys()
            .map(|param_id| params.get(*param_id).map(|value| (*param_id, value)))
            .collect::<Result<BTreeMap<clap_id, f64>>>()?;

        // The same param set events will be passed to the flush function in this pass and to the
        // process fuction in the second pass
        let param_fuzzer = ParamFuzzer::new(&param_infos);
        let random_param_set_events: Vec<_> =
            param_fuzzer.randomize_params_at(&mut prng, 0).collect();

        let input_events = EventQueue::new_input();
        *input_events.events.lock() = random_param_set_events.clone();
        let output_events = EventQueue::new_output();
        params.flush(&input_events, &output_events);
        host.handle_callbacks_once();

        // We'll compare against these values in that second pass
        let expected_param_values: BTreeMap<clap_id, f64> = param_infos
            .keys()
            .map(|param_id| params.get(*param_id).map(|value| (*param_id, value)))
            .collect::<Result<BTreeMap<clap_id, f64>>>()?;
        let expected_state = state.save()?;
        host.handle_callbacks_once();

        // Plugins with no parameters at all should of course not trigger this error
        if expected_param_values == initial_param_values && !param_infos.is_empty() {
            anyhow::bail!(
                "'clap_plugin_params::flush()' has been called with random parameter values, but \
                 the plugin's reported parameter values have not changed."
            )
        }

        (
            expected_state,
            random_param_set_events,
            expected_param_values,
        )
    };

    // This works the same as the basic state reproducibility test, except that we load the values
    // using the process funciton instead of loading the state
    drop(plugin);

    let plugin = library
        .create_plugin(plugin_id, host.clone())
        .context("Could not create the plugin instance a second time")?;
    plugin
        .init()
        .context("Error while initializing the second plugin instance")?;

    let audio_ports_config = match plugin.get_extension::<AudioPorts>() {
        Some(audio_ports) => audio_ports
            .config()
            .context("Error while querying 'audio-ports' IO configuration")?,
        None => AudioPortConfig::default(),
    };
    let params = match plugin.get_extension::<Params>() {
        Some(params) => params,
        None => {
            // I sure hope that no plugin will eer hit this
            return Ok(TestStatus::Skipped {
                details: Some(String::from(
                    "The plugin's second instance does not support the 'params' extension.",
                )),
            });
        }
    };
    let state = match plugin.get_extension::<State>() {
        Some(state) => state,
        None => {
            return Ok(TestStatus::Skipped {
                details: Some(String::from(
                    "The plugin's second instance does not support the 'state' extension.",
                )),
            })
        }
    };
    host.handle_callbacks_once();

    // NOTE: We can reuse random parameter set events, except that the cookie pointers may be
    //       different if the plugin uses those. So we need to update these cookies first.
    let param_infos = params
        .info()
        .context("Failure while fetching the plugin's parameters")?;
    let mut new_random_param_set_events = old_random_param_set_events;
    for event in new_random_param_set_events.iter_mut() {
        match event {
            Event::ParamValue(event) => {
                event.cookie = param_infos
                    .get(&event.param_id)
                    .with_context(|| {
                        format!(
                            "Expected the plugin to have a parameter with ID {}, but the \
                             parameter is missing",
                            event.param_id,
                        )
                    })?
                    .cookie;
            }
            event => panic!("Unexpected event {event:?}, this is a clap-validator bug"),
        }
    }

    // In theprevious pass we used flush, and here we use the process funciton
    let (mut input_buffers, mut output_buffers) = audio_ports_config.create_buffers(512);
    ProcessingTest::new_out_of_place(&plugin, &mut input_buffers, &mut output_buffers)?.run_once(
        ProcessConfig::default(),
        move |process_data| {
            *process_data.input_events.events.lock() = new_random_param_set_events;

            Ok(())
        },
    )?;

    let actual_param_values: BTreeMap<clap_id, f64> = expected_param_values
        .keys()
        .map(|param_id| params.get(*param_id).map(|value| (*param_id, value)))
        .collect::<Result<BTreeMap<clap_id, f64>>>()?;
    if actual_param_values != expected_param_values {
        let param_infos = params
            .info()
            .context("Failure while fetching the plugin's parameters")?;

        anyhow::bail!(
            "Setting the same parameter values through 'clap_plugin_params::flush()' and through \
             the process funciton results in different reported values when queried through \
             'clap_plugin_params::get_value()'. The mismatching values are {}.",
            format_mismatching_values(actual_param_values, &expected_param_values, &param_infos)
        );
    }

    let actual_state = state.save()?;
    host.handle_callbacks_once();

    host.thread_safety_check()
        .context("Thread safety checks failed")?;
    if actual_state == expected_state {
        Ok(TestStatus::Success { details: None })
    } else {
        let (expected_state_file_path, mut expected_state_file) =
            PluginTestCase::FlushStateReproducibility
                .temporary_file(plugin_id, EXPECTED_STATE_FILE_NAME)?;
        let (actual_state_file_path, mut actual_state_file) =
            PluginTestCase::FlushStateReproducibility
                .temporary_file(plugin_id, ACTUAL_STATE_FILE_NAME)?;

        expected_state_file.write_all(&expected_state)?;
        actual_state_file.write_all(&actual_state)?;

        anyhow::bail!(
            "Sending the same parameter values to two different instances of the plugin resulted \
             in different state files. Expected: '{}'. Actual: '{}'.",
            expected_state_file_path.display(),
            actual_state_file_path.display(),
        )
    }
}

/// The test for `PluginTestCase::BufferedStateStreams`.
pub fn test_buffered_state_streams(library: &PluginLibrary, plugin_id: &str) -> Result<TestStatus> {
    let mut prng = new_prng();

    let host = Host::new();
    let plugin = library
        .create_plugin(plugin_id, host.clone())
        .context("Could not create the plugin instance")?;

    let (expected_state, expected_param_values) = {
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
                    details: Some(String::from(
                        "The plugin does not support the 'params' extension.",
                    )),
                })
            }
        };
        let state = match plugin.get_extension::<State>() {
            Some(state) => state,
            None => {
                return Ok(TestStatus::Skipped {
                    details: Some(String::from(
                        "The plugin does not support the 'state' extension.",
                    )),
                })
            }
        };
        host.handle_callbacks_once();

        let param_infos = params
            .info()
            .context("Failure while fetching the plugin's parameters")?;
        let param_fuzzer = ParamFuzzer::new(&param_infos);
        let random_param_set_events: Vec<_> =
            param_fuzzer.randomize_params_at(&mut prng, 0).collect();
        let (mut input_buffers, mut output_buffers) = audio_ports_config.create_buffers(512);
        ProcessingTest::new_out_of_place(&plugin, &mut input_buffers, &mut output_buffers)?
            .run_once(ProcessConfig::default(), move |process_data| {
                *process_data.input_events.events.lock() = random_param_set_events;

                Ok(())
            })?;

        let expected_param_values: BTreeMap<clap_id, f64> = param_infos
            .keys()
            .map(|param_id| params.get(*param_id).map(|value| (*param_id, value)))
            .collect::<Result<BTreeMap<clap_id, f64>>>()?;

        // This state file is saved without buffered writes. It's expected that the plugin
        // implementsq this correctly, so we can check if it handles buffered streams correctly by
        // treating this as the ground truth.
        let expected_stae = state.save()?;
        host.handle_callbacks_once();

        (expected_stae, expected_param_values)
    };

    // Now we'll recreate the plugin instance, load the state using buffered reads, check the
    // parameter values, save it again using buffered writes, and then check whether the fir.
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
            return Ok(TestStatus::Skipped {
                details: Some(String::from(
                    "The plugin's second instance does not support the 'params' extension.",
                )),
            });
        }
    };
    let state = match plugin.get_extension::<State>() {
        Some(state) => state,
        None => {
            return Ok(TestStatus::Skipped {
                details: Some(String::from(
                    "The plugin's second instance does not support the 'state' extension.",
                )),
            })
        }
    };
    host.handle_callbacks_once();

    // This is a buffered load that only loads 17 bytes at a time. Why 17? Because.
    const BUFFERED_LOAD_MAX_BYTES: usize = 17;
    state.load_buffered(&expected_state, BUFFERED_LOAD_MAX_BYTES)?;
    host.handle_callbacks_once();

    let actual_param_values: BTreeMap<clap_id, f64> = expected_param_values
        .keys()
        .map(|param_id| params.get(*param_id).map(|value| (*param_id, value)))
        .collect::<Result<BTreeMap<clap_id, f64>>>()?;
    if actual_param_values != expected_param_values {
        let param_infos = params
            .info()
            .context("Failure while fetching the plugin's parameters")?;

        // To avoid flooding the output too much, we'll print only the different
        // values
        anyhow::bail!(
            "After reloading the state by allowing the plugin to read at most \
             {BUFFERED_LOAD_MAX_BYTES} bytes at a time, the plugin's parameter values do not \
             match the old values when queried through 'clap_plugin_params::get()'. The \
             mismatching values are {}.",
            format_mismatching_values(actual_param_values, &expected_param_values, &param_infos)
        );
    }

    // Because we're mean, we'll use a different prime number for the saving
    const BUFFERED_SAVE_MAX_BYTES: usize = 23;
    let actual_state = state.save_buffered(BUFFERED_SAVE_MAX_BYTES)?;
    host.handle_callbacks_once();

    host.thread_safety_check()
        .context("Thread safety checks failed")?;
    if actual_state == expected_state {
        Ok(TestStatus::Success { details: None })
    } else {
        let (expected_state_file_path, mut expected_state_file) =
            PluginTestCase::BufferedStateStreams
                .temporary_file(plugin_id, EXPECTED_STATE_FILE_NAME)?;
        let (actual_state_file_path, mut actual_state_file) = PluginTestCase::BufferedStateStreams
            .temporary_file(plugin_id, ACTUAL_STATE_FILE_NAME)?;

        expected_state_file.write_all(&expected_state)?;
        actual_state_file.write_all(&actual_state)?;

        anyhow::bail!(
            "Re-saving the loaded state resulted in a different state file. The original state \
             file being compared to was written unbuffered, reloaded by allowing the plugin to \
             read only {BUFFERED_LOAD_MAX_BYTES} bytes at a time, and then written again by \
             allowing the plugin to write only {BUFFERED_SAVE_MAX_BYTES} bytes at a time. \
             Expected: '{}'. Actual: '{}'.",
            expected_state_file_path.display(),
            actual_state_file_path.display(),
        )
    }
}

/// Build a string containing all different values between two sets of values.
///
/// # Panics
///
/// If the parameters in `actual_param_values` don't have corresponding entries in
/// `expected_param_values` and `param_infos`.
fn format_mismatching_values(
    actual_param_values: BTreeMap<clap_id, f64>,
    expected_param_values: &BTreeMap<clap_id, f64>,
    param_infos: &ParamInfo,
) -> String {
    actual_param_values
        .into_iter()
        .filter_map(|(param_id, actual_value)| {
            let expected_value = expected_param_values[&param_id];
            if actual_value == expected_value {
                None
            } else {
                let param_name = &param_infos[&param_id].name;
                Some(format!(
                    "parameter {param_id} ('{param_name}'), expected {expected_value:?}, actual \
                     {actual_value:?}"
                ))
            }
        })
        .collect::<Vec<String>>()
        .join(", ")
}
