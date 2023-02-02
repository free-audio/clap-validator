//! Tests that focus on parameters.

use anyhow::{Context, Result};
use clap_sys::events::CLAP_EVENT_PARAM_VALUE;
use clap_sys::id::clap_id;
use rand::Rng;
use serde::Serialize;
use std::collections::BTreeMap;

use super::processing::ProcessingTest;
use super::PluginTestCase;
use crate::host::Host;
use crate::plugin::ext::audio_ports::{AudioPortConfig, AudioPorts};
use crate::plugin::ext::note_ports::NotePorts;
use crate::plugin::ext::params::Params;
use crate::plugin::instance::process::{Event, ProcessConfig};
use crate::plugin::library::PluginLibrary;
use crate::tests::rng::{new_prng, NoteGenerator, ParamFuzzer};
use crate::tests::{TestCase, TestStatus};

/// The fixed buffer size to use for these tests.
const BUFFER_SIZE: usize = 512;
/// The number of different parameter combinations to try in the parameter fuzzing tests.
pub const FUZZ_NUM_PERMUTATIONS: usize = 50;
/// How many buffers of [`BUFFER_SIZE`] samples to process at each parameter permutation. This
/// allows the plugin's state to settle in before moving to the next set of parameter values.
pub const FUZZ_RUNS_PER_PERMUTATION: usize = 5;

/// The file name we'll use to dump the previous parameter values when a fuzzing test fails.
const PREVIOUS_PARAM_VALUES_FILE_NAME: &str = "param-values-previous.json";
/// The file name we'll use to dump the current parameter values when a fuzzing test fails.
const CURRENT_PARAM_VALUES_FILE_NAME: &str = "param-values-current.json";

/// The format parameter values will be written in when the fuzzing test fails. Used only for
/// serialization.
#[derive(Debug, Serialize)]
struct ParamValue<'a> {
    id: clap_id,
    name: &'a str,
    value: f64,
}

/// The test for `ProcessingTest::ParamConversions`.
pub fn test_param_conversions(library: &PluginLibrary, plugin_id: &str) -> Result<TestStatus> {
    let mut prng = new_prng();

    let host = Host::new();
    let plugin = library
        .create_plugin(plugin_id, host.clone())
        .context("Could not create the plugin instance")?;
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
    host.handle_callbacks_once();

    let param_infos = params
        .info()
        .context("Failure while fetching the plugin's parameters")?;

    // We keep track of how many parameters support these conversions. A plugin
    // should support either conversion either for all of its parameters, or for
    // none of them.
    const VALUES_PER_PARAM: usize = 6;
    let expected_conversions = param_infos.len() * VALUES_PER_PARAM;

    let mut num_supported_value_to_text = 0;
    let mut num_supported_text_to_value = 0;
    let mut failed_value_to_text_calls: Vec<(String, f64)> = Vec::new();
    let mut failed_text_to_value_calls: Vec<(String, String)> = Vec::new();
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
        'value_loop: for starting_value in values {
            // If the plugin rounds string representations then `value` may very
            // will not roundtrip correctly, so we'll start at the string
            // representation
            let starting_text = match params.value_to_text(param_id, starting_value)? {
                Some(text) => text,
                None => {
                    failed_value_to_text_calls.push((param_name.to_owned(), starting_value));
                    continue 'param_loop;
                }
            };
            num_supported_value_to_text += 1;
            let reconverted_value = match params.text_to_value(param_id, &starting_text)? {
                Some(value) => value,
                // We can't test text to value conversions without a text
                // value provided by the plugin, but if the plugin doesn't
                // support this then we should still continue testing
                // whether the value to text conversion works consistently
                None => {
                    failed_text_to_value_calls.push((param_name.to_owned(), starting_text));
                    continue 'value_loop;
                }
            };
            num_supported_text_to_value += 1;

            let reconverted_text = params
                .value_to_text(param_id, reconverted_value)?
                .with_context(|| {
                    format!(
                        "Failure in repeated value to text conversion for parameter {param_id} \
                         ('{param_name}')"
                    )
                })?;
            // Both of these are produced by the plugin, so they should be equal
            if starting_text != reconverted_text {
                anyhow::bail!(
                    "Converting {starting_value:?} to a string, back to a value, and then back to \
                     a string again for parameter {param_id} ('{param_name}') results in \
                     '{starting_text}' -> {reconverted_value:?} -> '{reconverted_text}', which is \
                     not consistent."
                );
            }

            // And one last hop back for good measure
            let final_value = params
                .text_to_value(param_id, &reconverted_text)?
                .with_context(|| {
                    format!(
                        "Failure in repeated text to value conversion for parameter {param_id} \
                         ('{param_name}')"
                    )
                })?;
            if final_value != reconverted_value {
                anyhow::bail!(
                    "Converting {starting_value:?} to a string, back to a value, back to a \
                     string, and then back to a value again for parameter {param_id} \
                     ('{param_name}') results in '{starting_text}' -> {reconverted_value:?} -> \
                     '{reconverted_text}' -> {final_value:?}, which is not consistent."
                );
            }
        }
    }

    if !(num_supported_value_to_text == 0 || num_supported_value_to_text == expected_conversions) {
        anyhow::bail!(
            "'clap_plugin_params::value_to_text()' returned true for \
             {num_supported_value_to_text} out of {expected_conversions} calls. This function is \
             expected to be supported for either none of the parameters or for all of them. \
             Examples of failing conversions were: {failed_value_to_text_calls:#?}"
        );
    }
    if !(num_supported_text_to_value == 0 || num_supported_text_to_value == expected_conversions) {
        anyhow::bail!(
            "'clap_plugin_params::text_to_value()' returned true for \
             {num_supported_text_to_value} out of {expected_conversions} calls. This function is \
             expected to be supported for either none of the parameters or for all of them. \
             Examples of failing conversions were: {failed_text_to_value_calls:#?}"
        );
    }

    host.thread_safety_check()
        .context("Thread safety checks failed")?;
    if num_supported_value_to_text == 0 || num_supported_text_to_value == 0 {
        Ok(TestStatus::Skipped {
            details: Some(String::from(
                "The plugin's parameters need to support both value to text and text to value \
                 conversions for this test.",
            )),
        })
    } else {
        Ok(TestStatus::Success { details: None })
    }
}

/// The test for `ProcessingTest::ParamFuzzBasic`.
pub fn test_param_fuzz_basic(library: &PluginLibrary, plugin_id: &str) -> Result<TestStatus> {
    let mut prng = new_prng();

    let host = Host::new();
    let plugin = library
        .create_plugin(plugin_id, host.clone())
        .context("Could not create the plugin instance")?;
    plugin.init().context("Error during initialization")?;

    // Both audio and note ports are optional
    let audio_ports = plugin.get_extension::<AudioPorts>();
    let note_ports = plugin.get_extension::<NotePorts>();
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
    host.handle_callbacks_once();

    let audio_ports_config = audio_ports
        .map(|ports| ports.config())
        .transpose()
        .context("Could not fetch the plugin's audio port config")?;
    let note_ports_config = note_ports
        .map(|ports| ports.config())
        .transpose()
        .context("Could not fetch the plugin's note port config")?
        // Don't try to generate notes if the plugin supports the note ports extension but doesn't
        // actually have any note ports. JUCE does this.
        .filter(|config| !config.inputs.is_empty());
    let param_infos = params
        .info()
        .context("Could not fetch the plugin's parameters")?;

    // For each set of runs we'll generate new parameter values, and if the plugin supports notes
    // we'll also generate note events.
    let param_fuzzer = ParamFuzzer::new(&param_infos);
    let mut note_event_rng = note_ports_config.map(NoteGenerator::new);

    // We'll keep track of the current and the previous set of parameter value so we can write them
    // to a file if the test fails
    let mut current_events: Option<Vec<Event>>;
    let mut previous_events: Option<Vec<Event>> = None;

    let (mut input_buffers, mut output_buffers) = audio_ports_config
        .unwrap_or_default()
        .create_buffers(BUFFER_SIZE);
    for permutation_no in 1..=FUZZ_NUM_PERMUTATIONS {
        current_events = Some(param_fuzzer.randomize_params_at(&mut prng, 0).collect());

        let mut have_set_parameters = false;
        let run_result =
            ProcessingTest::new_out_of_place(&plugin, &mut input_buffers, &mut output_buffers)?
                .run(
                    FUZZ_RUNS_PER_PERMUTATION,
                    ProcessConfig::default(),
                    |process_data| {
                        if !have_set_parameters {
                            *process_data.input_events.events.lock() =
                                current_events.clone().unwrap();
                            have_set_parameters = true;
                        }

                        // Audio and MIDI/note events are randomized in accordance to what the plugin
                        // supports
                        if let Some(note_event_rng) = note_event_rng.as_mut() {
                            // This includes a sort if `random_param_set_events` also contained a queue
                            note_event_rng.fill_event_queue(
                                &mut prng,
                                &process_data.input_events,
                                BUFFER_SIZE as u32,
                            )?;
                        }
                        process_data.buffers.randomize(&mut prng);

                        Ok(())
                    },
                );

        // If the run failed we'll want to write the parameter values to a file first
        if run_result.is_err() {
            let (previous_param_values_file_path, previous_param_values_file) =
                PluginTestCase::ParamFuzzBasic
                    .temporary_file(plugin_id, PREVIOUS_PARAM_VALUES_FILE_NAME)?;
            let (current_param_values_file_path, current_param_values_file) =
                PluginTestCase::ParamFuzzBasic
                    .temporary_file(plugin_id, CURRENT_PARAM_VALUES_FILE_NAME)?;

            let create_param_values_vec = |events: Option<Vec<Event>>| match events {
                Some(events) => events
                    .into_iter()
                    .map(|event| match event {
                        Event::ParamValue(event) => ParamValue {
                            id: event.param_id,
                            name: &param_infos[&event.param_id].name,
                            value: event.value,
                        },
                        _ => panic!("Unexpected event type. This is a clap-validator bug."),
                    })
                    .collect(),
                None => Vec::new(),
            };
            let previous_param_values: Vec<ParamValue> = create_param_values_vec(previous_events);
            let current_param_values: Vec<ParamValue> = create_param_values_vec(current_events);

            serde_json::to_writer_pretty(previous_param_values_file, &previous_param_values)?;
            serde_json::to_writer_pretty(current_param_values_file, &current_param_values)?;

            // This is a bit weird and there may be a better way to do this, but we only want to
            // write the parameter values if we know the run has failed, and we only know the
            // filename after writing those values to a file
            return Err(run_result
                .with_context(|| {
                    format!(
                        "Invalid output detected in parameter value permutation {} of {} ('{}' \
                         and '{}' contain the current and previous parameter values)",
                        permutation_no,
                        FUZZ_NUM_PERMUTATIONS,
                        current_param_values_file_path.display(),
                        previous_param_values_file_path.display(),
                    )
                })
                .unwrap_err());
        }

        std::mem::swap(&mut previous_events, &mut current_events);
    }

    // `ProcessingTest::run()` already handled callbacks for us
    host.thread_safety_check()
        .context("Thread safety checks failed")?;

    Ok(TestStatus::Success { details: None })
}

/// The test for `ProcessingTest::ParamSetWrongNamespace`.
pub fn test_param_set_wrong_namespace(
    library: &PluginLibrary,
    plugin_id: &str,
) -> Result<TestStatus> {
    let mut prng = new_prng();

    let host = Host::new();
    let plugin = library
        .create_plugin(plugin_id, host.clone())
        .context("Could not create the plugin instance")?;
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
    host.handle_callbacks_once();

    let param_infos = params
        .info()
        .context("Failure while fetching the plugin's parameters")?;
    let initial_param_values: BTreeMap<clap_id, f64> = param_infos
        .keys()
        .map(|param_id| params.get(*param_id).map(|value| (*param_id, value)))
        .collect::<Result<BTreeMap<clap_id, f64>>>()?;

    // We'll generate random parameter set events, but we'll change the namespace ID to something
    // else. The plugin's parameter values should thus not update its parameter values.
    const INCORRECT_NAMESPACE_ID: u16 = 0xb33f;
    let param_fuzzer = ParamFuzzer::new(&param_infos);
    let mut random_param_set_events: Vec<_> =
        param_fuzzer.randomize_params_at(&mut prng, 0).collect();
    for event in random_param_set_events.iter_mut() {
        match event {
            Event::ParamValue(event) => event.header.space_id = INCORRECT_NAMESPACE_ID,
            event => panic!("Unexpected event {event:?}, this is a clap-validator bug"),
        }
    }

    let (mut input_buffers, mut output_buffers) = audio_ports_config.create_buffers(BUFFER_SIZE);
    ProcessingTest::new_out_of_place(&plugin, &mut input_buffers, &mut output_buffers)?.run_once(
        ProcessConfig::default(),
        move |process_data| {
            *process_data.input_events.events.lock() = random_param_set_events;

            Ok(())
        },
    )?;

    // We'll check that the plugin has these sames values after reloading the state. These values
    // are rounded to the tenth decimal to provide some leeway in the serialization and
    // deserializatoin process.
    let actual_param_values: BTreeMap<clap_id, f64> = param_infos
        .keys()
        .map(|param_id| params.get(*param_id).map(|value| (*param_id, value)))
        .collect::<Result<BTreeMap<clap_id, f64>>>()?;

    host.thread_safety_check()
        .context("Thread safety checks failed")?;
    if actual_param_values == initial_param_values {
        Ok(TestStatus::Success { details: None })
    } else {
        Ok(TestStatus::Failed {
            details: Some(format!(
                "Sending events with type ID {CLAP_EVENT_PARAM_VALUE} (CLAP_EVENT_PARAM_VALUE) \
                 and namespace ID {INCORRECT_NAMESPACE_ID:#x} to the plugin caused its parameter \
                 values to change. This should not happen. The plugin may not be checking the \
                 event's namespace ID."
            )),
        })
    }
}
