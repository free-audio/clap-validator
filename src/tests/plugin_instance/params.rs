//! Tests that focus on parameters.

use super::PluginInstanceTestCase;
use crate::cli::tracing::{Span, record};
use crate::plugin::ext::audio_ports::{AudioPortConfig, AudioPorts};
use crate::plugin::ext::note_ports::{NotePortConfig, NotePorts};
use crate::plugin::ext::params::{Param, ParamInfo, Params};
use crate::plugin::library::PluginLibrary;
use crate::plugin::process::{AudioBuffers, Event, InputEventQueue, OutputEventQueue, ProcessScope};
use crate::tests::rng::{NoteGenerator, ParamFuzzer, new_prng};
use crate::tests::{TestStatus, temporary_file};
use anyhow::{Context, Result};
use clap_sys::events::CLAP_EVENT_PARAM_VALUE;
use clap_sys::id::clap_id;
use serde::Serialize;
use std::collections::BTreeMap;
use std::ptr::null_mut;

/// The fixed buffer size to use for these tests.
const BUFFER_SIZE: u32 = 512;
/// The number of different parameter combinations to try in the parameter fuzzing tests.
pub const FUZZ_NUM_PERMUTATIONS: usize = 50;
/// How many buffers of [`BUFFER_SIZE`] samples to process at each parameter permutation. This
/// allows the state to settle in before moving to the next set of parameter values.
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

impl<'a> ParamValue<'a> {
    fn from_events(events: Option<Vec<Event>>, param_info: &'a ParamInfo) -> Vec<Self> {
        events
            .into_iter()
            .flatten()
            .map(|event| match event {
                Event::ParamValue(event) => ParamValue {
                    id: event.param_id,
                    name: &param_info[&event.param_id].name,
                    value: event.value,
                },
                _ => panic!("Unexpected event type"),
            })
            .collect()
    }
}

/// The test for `ProcessingTest::ParamConversions`.
pub fn test_param_conversions(library: &PluginLibrary, plugin_id: &str) -> Result<TestStatus> {
    let plugin = library
        .create_plugin(plugin_id)
        .context("Could not create the plugin instance")?;
    plugin.init().context("Error during initialization")?;

    let params = match plugin.get_extension::<Params>() {
        Some(params) => params,
        None => {
            return Ok(TestStatus::Skipped {
                details: Some(String::from("The plugin does not implement the 'params' extension.")),
            });
        }
    };

    plugin.poll_callback(|_| Ok(()))?;

    let param_info = params.info().context("Failure while fetching the parameters")?;

    // We keep track of how many parameters support these conversions. A plugin
    // should support either conversion either for all of its parameters, or for
    // none of them.

    let conversions_per_param = 4000usize.div_ceil(param_info.len()).clamp(5, 100);
    let expected_conversions = param_info.len() * conversions_per_param;

    let mut num_supported_value_to_text = 0;
    let mut num_supported_text_to_value = 0;
    let mut failed_value_to_text_calls: Vec<(String, f64)> = Vec::new();
    let mut failed_text_to_value_calls: Vec<(String, String)> = Vec::new();

    'param_loop: for (param_id, param_info) in param_info {
        let param_name = &param_info.name;
        let _span = Span::begin("Param", record! { param_id: param_id, param_name: param_name });

        'value_loop: for i in 0..conversions_per_param {
            let starting_value = param_info.range.start()
                + (param_info.range.end() - param_info.range.start()) * (i as f64 / (conversions_per_param - 1) as f64);

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

            let reconverted_text = params.value_to_text(param_id, reconverted_value)?.with_context(|| {
                format!("Failure in repeated value to text conversion for parameter {param_id} ('{param_name}')")
            })?;
            // Both of these are produced by the plugin, so they should be equal
            if starting_text != reconverted_text {
                anyhow::bail!(
                    "Converting {starting_value:?} to a string, back to a value, and then back to a string again for \
                     parameter '{param_name}' ({param_id}) results in '{starting_text}' -> {reconverted_value:?} -> \
                     '{reconverted_text}', which is not consistent."
                );
            }

            // And one last hop back for good measure
            let final_value = params.text_to_value(param_id, &reconverted_text)?.with_context(|| {
                format!("Failure in repeated text to value conversion for parameter {param_id} ('{param_name}')")
            })?;
            if final_value != reconverted_value {
                anyhow::bail!(
                    "Converting {starting_value:?} to a string, back to a value, back to a string, and then back to a \
                     value again for parameter '{param_name}' ({param_id}) results in '{starting_text}' -> \
                     {reconverted_value:?} -> '{reconverted_text}' -> {final_value:?}, which is not consistent."
                );
            }
        }
    }

    plugin.poll_callback(|_| Ok(()))?;

    if num_supported_value_to_text == 0 || num_supported_text_to_value == 0 {
        return Ok(TestStatus::Success {
            details: Some(String::from(
                "The plugin does not support text-to-value and value-to-text parameter conversions",
            )),
        });
    }

    if num_supported_value_to_text != expected_conversions {
        let failed_value_to_text_calls = failed_value_to_text_calls
            .into_iter()
            .take(10)
            .map(|(name, value)| format!("\n - {name}: {value:.4}"))
            .collect::<Vec<_>>()
            .join("");

        anyhow::bail!(
            "'clap_plugin_params::value_to_text()' returned true for {num_supported_value_to_text} out of \
             {expected_conversions} calls. This function is expected to be supported for either none of the \
             parameters or for all of them. Examples of failing conversions were: {failed_value_to_text_calls}"
        );
    }

    if num_supported_text_to_value != expected_conversions {
        let failed_text_to_value_calls = failed_text_to_value_calls
            .into_iter()
            .take(10)
            .map(|(name, text)| format!("\n - {name}: '{text}'"))
            .collect::<Vec<_>>()
            .join("");

        anyhow::bail!(
            "'clap_plugin_params::text_to_value()' returned true for {num_supported_text_to_value} out of \
             {expected_conversions} calls. This function is expected to be supported for either none of the \
             parameters or for all of them. Examples of failing conversions were: {failed_text_to_value_calls}"
        );
    }

    Ok(TestStatus::Success { details: None })
}

/// The test for `ProcessingTest::ParamChangeEvents`.
pub fn test_param_set_events(library: &PluginLibrary, plugin_id: &str, null_cookies: bool) -> Result<TestStatus> {
    // first, flush run
    let span = Span::begin("FlushRun", ());

    let plugin = library
        .create_plugin(plugin_id)
        .context("Could not create the plugin instance")?;
    plugin.init().context("Error during initialization")?;

    let params = match plugin.get_extension::<Params>() {
        Some(params) => params,
        None => {
            return Ok(TestStatus::Skipped {
                details: Some(String::from("The plugin does not implement the 'params' extension.")),
            });
        }
    };

    let param_info = params.info().context("Failure while fetching the parameters")?;
    let mut param_events = ParamFuzzer::new(&param_info)
        .randomize_params_at(&mut new_prng(), 0)
        .collect::<Vec<_>>();

    if param_events.is_empty() {
        return Ok(TestStatus::Skipped {
            details: Some(String::from("The plugin does not have any automatable parameters")),
        });
    }

    if null_cookies {
        for event in param_events.iter_mut() {
            match event {
                Event::ParamValue(event) => event.cookie = null_mut(),
                event => panic!("Unexpected event {event:?}"),
            }
        }
    }

    let flush_param_values = {
        let initial_param_values = param_get_values(&params)?;

        plugin.poll_callback(|_| Ok(()))?;

        let input_queue = InputEventQueue::new();
        input_queue.add_events(param_events.iter().cloned());
        params.flush(&input_queue, &OutputEventQueue::new());

        plugin.poll_callback(|_| Ok(()))?;

        let flush_param_values = param_get_values(&params)?;
        if flush_param_values == initial_param_values {
            anyhow::bail!("After calling 'clap_plugin_params::flush()', the parameter values did not change");
        }

        flush_param_values
    };

    span.finish(());

    // second run, use process this time
    let span = Span::begin("ProcessRun", ());

    let plugin = library
        .create_plugin(plugin_id)
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
        None => anyhow::bail!("The second instance does not implement the 'params' extension."),
    };

    // we have to recreate the events because of cookies (they can be different between plugin instances)
    let param_info = params.info().context("Failure while fetching the parameters")?;
    let mut param_events = ParamFuzzer::new(&param_info)
        .with_no_cookies(null_cookies)
        .randomize_params_at(&mut new_prng(), 0)
        .collect::<Vec<_>>();

    if null_cookies {
        for event in param_events.iter_mut() {
            match event {
                Event::ParamValue(event) => event.cookie = null_mut(),
                event => panic!("Unexpected event {event:?}"),
            }
        }
    }

    let process_param_values = {
        let initial_param_values = param_get_values(&params)?;

        plugin.poll_callback(|_| Ok(()))?;

        plugin.on_audio_thread(|plugin| {
            let mut buffers = AudioBuffers::new_out_of_place_f32(&audio_ports_config, BUFFER_SIZE);
            let mut process = ProcessScope::new(&plugin, &mut buffers)?;

            plugin.poll_callback();
            process.add_events(param_events);
            process.run()
        })?;

        plugin.poll_callback(|_| Ok(()))?;

        let process_param_values = param_get_values(&params)?;
        if process_param_values == initial_param_values {
            anyhow::bail!(
                "After sending parameter changes via 'clap_plugin::process()', the parameter values did not change"
            );
        }

        process_param_values
    };

    span.finish(());

    if let Some(diff) = param_generate_diff(&flush_param_values, &process_param_values, &params)? {
        anyhow::bail!(
            "The resulting parameter values after calling 'clap_plugin_params::flush()' were different from the \
             resulting parameter values after sending the same parameter changes via 'clap_plugin::process()': \n{}",
            diff
        );
    }

    Ok(TestStatus::Success { details: None })
}

/// The test for `ProcessingTest::ParamFuzzBasic` and `ProcessingTest::ParamFuzzBounds`.
pub fn test_param_fuzz_basic(library: &PluginLibrary, plugin_id: &str, snap_to_bounds: bool) -> Result<TestStatus> {
    let mut prng = new_prng();
    let plugin = library
        .create_plugin(plugin_id)
        .context("Could not create the plugin instance")?;
    plugin.init().context("Error during initialization")?;

    // Both audio and note ports are optional
    let audio_ports = plugin.get_extension::<AudioPorts>();
    let note_ports = plugin.get_extension::<NotePorts>();
    let params = match plugin.get_extension::<Params>() {
        Some(params) => params,
        None => {
            return Ok(TestStatus::Skipped {
                details: Some(String::from("The plugin does not implement the 'params' extension.")),
            });
        }
    };

    plugin.poll_callback(|_| Ok(()))?;

    let audio_ports_config = audio_ports
        .map(|ports| ports.config())
        .transpose()
        .context("Could not fetch the audio port config")?
        .unwrap_or_default();
    let note_ports_config = note_ports
        .map(|ports| ports.config())
        .transpose()
        .context("Could not fetch the note port config")?
        .unwrap_or_default();

    // For each set of runs we'll generate new parameter values, and if the plugin supports notes
    // we'll also generate note events.
    let param_info = params.info().context("Could not fetch the parameters")?;
    let param_fuzzer = ParamFuzzer::new(&param_info).snap_to_bounds(snap_to_bounds);
    let mut note_rng = NoteGenerator::new(&note_ports_config).with_sample_offset_range(-1..=128);

    // We'll keep track of the current and the previous set of parameter value so we can write them
    // to a file if the test fails
    let mut current_events: Option<Vec<Event>>;
    let mut previous_events: Option<Vec<Event>> = None;
    let mut audio_buffers = AudioBuffers::new_out_of_place_f32(&audio_ports_config, BUFFER_SIZE);

    for permutation_no in 1..=FUZZ_NUM_PERMUTATIONS {
        current_events = Some(param_fuzzer.randomize_params_at(&mut prng, 0).collect());

        let run_result = plugin.on_audio_thread(|plugin| -> Result<()> {
            let mut process = ProcessScope::new(&plugin, &mut audio_buffers)?;

            process.add_events(current_events.clone().unwrap());

            for _ in 0..FUZZ_RUNS_PER_PERMUTATION {
                plugin.poll_callback();
                process.audio_buffers().fill_white_noise(&mut prng);
                process.add_events(note_rng.generate_events(&mut prng, BUFFER_SIZE));
                process.run()?;
            }

            Ok(())
        });

        // If the run failed we'll want to write the parameter values to a file first
        if run_result.is_err() {
            let (previous_param_values_file_path, previous_param_values_file) = temporary_file(
                &PluginInstanceTestCase::ParamFuzzBasic.to_string(),
                plugin_id,
                PREVIOUS_PARAM_VALUES_FILE_NAME,
            )?;

            let (current_param_values_file_path, current_param_values_file) = temporary_file(
                &PluginInstanceTestCase::ParamFuzzBasic.to_string(),
                plugin_id,
                CURRENT_PARAM_VALUES_FILE_NAME,
            )?;

            serde_json::to_writer_pretty(
                previous_param_values_file,
                &ParamValue::from_events(previous_events, &param_info),
            )?;
            serde_json::to_writer_pretty(
                current_param_values_file,
                &ParamValue::from_events(current_events, &param_info),
            )?;

            // This is a bit weird and there may be a better way to do this, but we only want to
            // write the parameter values if we know the run has failed, and we only know the
            // filename after writing those values to a file
            return Err(run_result
                .with_context(|| {
                    format!(
                        "Invalid output detected in parameter value permutation {} of {} ('{}' and '{}' contain the \
                         current and previous parameter values)",
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

    plugin.poll_callback(|_| Ok(()))?;

    Ok(TestStatus::Success { details: None })
}

/// The test for `ProcessingTest::ParamFuzzSampleAccurate`.
pub fn test_param_fuzz_sample_accurate(library: &PluginLibrary, plugin_id: &str) -> Result<TestStatus> {
    const INTERVALS: &[u32] = &[1000, 100, 10];

    let mut prng = new_prng();
    let plugin = library
        .create_plugin(plugin_id)
        .context("Could not create the plugin instance")?;
    plugin.init().context("Error during initialization")?;

    // Both audio and note ports are optional
    let audio_ports = plugin.get_extension::<AudioPorts>();
    let note_ports = plugin.get_extension::<NotePorts>();
    let params = match plugin.get_extension::<Params>() {
        Some(params) => params,
        None => {
            return Ok(TestStatus::Skipped {
                details: Some(String::from("The plugin does not implement the 'params' extension.")),
            });
        }
    };

    plugin.poll_callback(|_| Ok(()))?;

    let audio_ports_config = audio_ports
        .map(|ports| ports.config())
        .transpose()
        .context("Could not fetch the audio port config")?
        .unwrap_or_default();

    let note_ports_config = note_ports
        .map(|ports| ports.config())
        .transpose()
        .context("Could not fetch the note port config")?
        .unwrap_or_default();

    let param_info = params.info().context("Could not fetch the parameters")?;

    // For each set of runs we'll generate new parameter values, and if the plugin supports notes
    // we'll also generate note events.
    let param_fuzzer = ParamFuzzer::new(&param_info);
    let mut note_rng = NoteGenerator::new(&note_ports_config).with_sample_offset_range(-1..=128);
    let mut current_events: Option<Vec<Event>> = None;
    let mut audio_buffers = AudioBuffers::new_out_of_place_f32(&audio_ports_config, BUFFER_SIZE);

    for &interval in INTERVALS {
        let _span = Span::begin("Interval", record! { interval: interval });
        let num_steps = (interval * 4).div_ceil(BUFFER_SIZE);

        plugin.on_audio_thread(|plugin| -> Result<()> {
            let mut process = ProcessScope::new(&plugin, &mut audio_buffers)?;
            let mut current_sample = 0;
            for _ in 0..num_steps {
                while current_sample < BUFFER_SIZE {
                    let events: Vec<Event> = param_fuzzer.randomize_params_at(&mut prng, current_sample).collect();
                    process.add_events(events.clone());
                    current_events = Some(events);
                    current_sample += interval;
                }

                current_sample -= BUFFER_SIZE;

                // Audio and MIDI/note events are randomized in accordance to what the plugin
                // supports
                plugin.poll_callback();
                process.audio_buffers().fill_white_noise(&mut prng);
                process.add_events(note_rng.generate_events(&mut prng, BUFFER_SIZE));
                process.run()?;
            }

            Ok(())
        })?;
    }

    plugin.poll_callback(|_| Ok(()))?;

    Ok(TestStatus::Success { details: None })
}

/// The test for `ProcessingTest::ParamFuzzModulation`.
pub fn test_param_fuzz_modulation(library: &PluginLibrary, plugin_id: &str) -> Result<TestStatus> {
    let mut prng = new_prng();
    let plugin = library
        .create_plugin(plugin_id)
        .context("Could not create the plugin instance")?;
    plugin.init().context("Error during initialization")?;

    let audio_ports = match plugin.get_extension::<AudioPorts>() {
        Some(audio_ports) => audio_ports.config().context("Could not fetch the audio port config")?,
        None => AudioPortConfig::default(),
    };

    let note_ports = match plugin.get_extension::<NotePorts>() {
        Some(note_ports) => note_ports.config().context("Could not fetch the note port config")?,
        None => NotePortConfig::default(),
    };

    let params = match plugin.get_extension::<Params>() {
        Some(params) => params,
        None => {
            return Ok(TestStatus::Skipped {
                details: Some(String::from("The plugin does not implement the 'params' extension.")),
            });
        }
    };

    let param_info = params.info().context("Could not fetch the parameters")?;
    let param_fuzzer = ParamFuzzer::new(&param_info);
    let mut note_rng = NoteGenerator::new(&note_ports).with_params(&param_info);
    let mut audio_buffers = AudioBuffers::new_out_of_place_f32(&audio_ports, BUFFER_SIZE);

    plugin.poll_callback(|_| Ok(()))?;

    plugin.on_audio_thread(|plugin| -> Result<()> {
        let mut process = ProcessScope::new(&plugin, &mut audio_buffers)?;

        plugin.poll_callback();
        process.audio_buffers().fill_white_noise(&mut prng);
        process.add_events(param_fuzzer.generate_events(&mut prng, process.max_block_size()));
        process.add_events(note_rng.generate_events(&mut prng, process.max_block_size()));
        process.run()?;

        Ok(())
    })?;

    plugin.poll_callback(|_| Ok(()))?;

    Ok(TestStatus::Success { details: None })
}

/// The test for `ProcessingTest::ParamSetWrongNamespace`.
pub fn test_param_set_wrong_namespace(library: &PluginLibrary, plugin_id: &str) -> Result<TestStatus> {
    let mut prng = new_prng();
    let plugin = library
        .create_plugin(plugin_id)
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
                details: Some(String::from("The plugin does not implement the 'params' extension.")),
            });
        }
    };

    plugin.poll_callback(|_| Ok(()))?;

    let param_info = params.info().context("Failure while fetching the parameters")?;
    let initial_param_values = param_get_values(&params)?;

    // We'll generate random parameter set events, but we'll change the namespace ID to something
    // else. The parameter values should thus not update its parameter values.
    const INCORRECT_NAMESPACE_ID: u16 = 0xb33f;
    let param_fuzzer = ParamFuzzer::new(&param_info);
    let mut random_param_set_events: Vec<_> = param_fuzzer.randomize_params_at(&mut prng, 0).collect();

    for event in random_param_set_events.iter_mut() {
        match event {
            Event::ParamValue(event) => event.header.space_id = INCORRECT_NAMESPACE_ID,
            event => panic!("Unexpected event {event:?}"),
        }
    }

    plugin.on_audio_thread(|plugin| {
        let mut buffers = AudioBuffers::new_out_of_place_f32(&audio_ports_config, BUFFER_SIZE);
        let mut process = ProcessScope::new(&plugin, &mut buffers)?;

        plugin.poll_callback();
        process.audio_buffers().fill_white_noise(&mut prng);
        process.add_events(random_param_set_events);
        process.run()
    })?;

    // We'll check that the plugin has these sames values after reloading the state. These values
    // are rounded to the tenth decimal to provide some leeway in the serialization and
    // deserialization process.
    let actual_param_values = param_get_values(&params)?;

    plugin.poll_callback(|_| Ok(()))?;

    if actual_param_values == initial_param_values {
        Ok(TestStatus::Success { details: None })
    } else {
        Ok(TestStatus::Failed {
            details: Some(format!(
                "Sending events with type ID {CLAP_EVENT_PARAM_VALUE} (CLAP_EVENT_PARAM_VALUE) and namespace ID \
                 {INCORRECT_NAMESPACE_ID:#x} to the plugin caused its parameter values to change. This should not \
                 happen. The plugin may not be checking the event's namespace ID."
            )),
        })
    }
}

/// The test for `ProcessingTest::ParamDefaultValues`.
pub fn test_param_default_values(library: &PluginLibrary, plugin_id: &str) -> Result<TestStatus> {
    let plugin = library
        .create_plugin(plugin_id)
        .context("Could not create the plugin instance")?;
    plugin.init().context("Error during initialization")?;

    let params = match plugin.get_extension::<Params>() {
        Some(params) => params,
        None => {
            return Ok(TestStatus::Skipped {
                details: Some(String::from("The plugin does not implement the 'params' extension.")),
            });
        }
    };

    plugin.poll_callback(|_| Ok(()))?;

    let param_info = params.info().context("Failure while fetching the parameters")?;

    for (param_id, param_info) in param_info {
        let default_value = params
            .get(param_id)
            .with_context(|| format!("Could not get value for parameter {param_id}"))?;

        if !param_compare_approx(&param_info, default_value, param_info.default) {
            anyhow::bail!(
                "The default value for parameter {param_id} ('{}') is {}, but the actual parameter value after \
                 initialization is {}.",
                param_info.name,
                param_info.default,
                default_value
            );
        }
    }

    plugin.poll_callback(|_| Ok(()))?;

    Ok(TestStatus::Success { details: None })
}

pub fn param_get_values(params: &Params) -> Result<BTreeMap<clap_id, f64>> {
    params
        .info()?
        .keys()
        .map(|param_id| params.get(*param_id).map(|value| (*param_id, value)))
        .collect::<Result<BTreeMap<clap_id, f64>>>()
}

pub fn param_compare_approx(param: &Param, actual: f64, expected: f64) -> bool {
    if param.stepped() {
        let actual = actual.round() as i64;
        let expected = expected.round() as i64;

        actual == expected
    } else {
        let actual = (actual - param.range.start()) / (param.range.end() - param.range.start());
        let expected = (expected - param.range.start()) / (param.range.end() - param.range.start());

        (actual - expected).abs() <= 1e-4 // 0.01% of the range
    }
}

/// Build a string containing differences between two sets of parameters, pretty formatted
pub fn param_generate_diff(
    actual: &BTreeMap<clap_id, f64>,
    expected: &BTreeMap<clap_id, f64>,
    params: &Params,
) -> Result<Option<String>> {
    let param_info = params.info()?;

    let mut diff = param_info
        .iter()
        .filter_map(|(param_id, info)| {
            let value_a = actual.get(param_id);
            let value_b = expected.get(param_id);

            let string_a = value_a.and_then(|&value| params.value_to_text(*param_id, value).ok().flatten());
            let string_b = value_b.and_then(|&value| params.value_to_text(*param_id, value).ok().flatten());

            // If we have strings, and they're equal, then we consider the parameters to be equal, even if the values are not exactly equal.
            // This is because some plugins may round parameter values when converting them to strings, and we want to allow for that.
            if let (Some(string_a), Some(string_b)) = (string_a.as_ref(), string_b.as_ref())
                && string_a == string_b
            {
                return None;
            }

            if let (Some(value_a), Some(value_b)) = (value_a, value_b)
                && param_compare_approx(info, *value_a, *value_b)
            {
                return None;
            }

            let print_a = match (string_a, value_a) {
                (Some(string_a), Some(value_a)) => format!("{} ({:.4})", string_a, value_a),
                (None, Some(value_a)) => format!("{:.4}", value_a),
                _ => "missing".to_string(),
            };

            let print_b = match (string_b, value_b) {
                (Some(string_b), Some(value_b)) => format!("{} ({:.4})", string_b, value_b),
                (None, Some(value_b)) => format!("{:.4}", value_b),
                _ => "missing".to_string(),
            };

            Some(format!(" - {} ({}) - {} vs {}", info.name, param_id, print_a, print_b))
        })
        .collect::<Vec<String>>();

    let num_diffs = diff.len();
    if num_diffs == 0 {
        Ok(None)
    } else if num_diffs > 5 {
        diff.truncate(5);
        Ok(Some(format!("{}\n...and {} more", diff.join("\n"), num_diffs - 5)))
    } else {
        Ok(Some(diff.join("\n")))
    }
}
