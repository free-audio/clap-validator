//! Tests that focus on parameters.

use anyhow::Context;
use rand::Rng;

use crate::host::Host;
use crate::plugin::ext::params::Params;
use crate::plugin::library::PluginLibrary;
use crate::tests::rng::new_prng;
use crate::tests::TestStatus;

/// The test for `ProcessingTest::ConvertParams`.
pub fn test_convert_params(library: &PluginLibrary, plugin_id: &str) -> TestStatus {
    let mut prng = new_prng();

    let host = Host::new();
    let result = library
        .create_plugin(plugin_id, host.clone())
        .context("Could not create the plugin instance")
        .and_then(|plugin| {
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
                        None => continue 'param_loop,
                    };
                    num_supported_value_to_text += 1;
                    let reconverted_value = match params.text_to_value(param_id, &starting_text)? {
                        Some(value) => value,
                        // We can't test text to value conversions without a text
                        // value provided by the plugin, but if the plugin doesn't
                        // support this then we should still continue testing
                        // whether the value to text conversion works consistently
                        None => continue 'value_loop,
                    };
                    num_supported_text_to_value += 1;

                    let reconverted_text = params
                        .value_to_text(param_id, reconverted_value)?
                        .with_context(|| {
                            format!(
                                "Failure in repeated value to text conversion for parameter \
                                 {param_id} ('{param_name}')"
                            )
                        })?;
                    // Both of these are produced by the plugin, so they should be equal
                    if starting_text != reconverted_text {
                        anyhow::bail!(
                            "Converting {starting_value:?} to a string, back to a value, and then \
                             back to a string again for parameter {param_id} ('{param_name}') \
                             results in '{starting_text}' -> {reconverted_value:?} -> \
                             '{reconverted_text}', which is not consistent."
                        );
                    }

                    // And one last hop back for good measure
                    let final_value = params
                        .text_to_value(param_id, &reconverted_text)?
                        .with_context(|| {
                            format!(
                                "Failure in repeated text to value conversion for parameter \
                                 {param_id} ('{param_name}')"
                            )
                        })?;
                    if final_value != reconverted_value {
                        anyhow::bail!(
                            "Converting {starting_value:?} to a string, back to a value, back to \
                             a string, and then back to a value again for parameter {param_id} \
                             ('{param_name}') results in '{starting_text}' -> \
                             {reconverted_value:?} -> '{reconverted_text}' -> {final_value:?}, \
                             which is not consistent."
                        );
                    }
                }
            }

            if !(num_supported_value_to_text == 0
                || num_supported_value_to_text == expected_conversions)
            {
                anyhow::bail!(
                    "'clap_plugin_params::value_to_text()' returned true for \
                     {num_supported_value_to_text} out of {expected_conversions} calls. This \
                     function is expected to be supported for either none of the parameters or \
                     for all of them."
                );
            }
            if !(num_supported_text_to_value == 0
                || num_supported_text_to_value == expected_conversions)
            {
                anyhow::bail!(
                    "'clap_plugin_params::text_to_value()' returned true for \
                     {num_supported_text_to_value} out of {expected_conversions} calls. This \
                     function is expected to be supported for either none of the parameters or \
                     for all of them."
                );
            }

            host.thread_safety_check()
                .context("Thread safety checks failed")?;

            if num_supported_value_to_text == 0 || num_supported_text_to_value == 0 {
                Ok(TestStatus::Skipped {
                    details: Some(String::from(
                        "The plugin's parameters need to support both value to text and text to \
                         value conversions for this test.",
                    )),
                })
            } else {
                Ok(TestStatus::Success { details: None })
            }
        });

    match result {
        Ok(status) => status,
        Err(err) => TestStatus::Failed {
            details: Some(format!("{err:#}")),
        },
    }
}
