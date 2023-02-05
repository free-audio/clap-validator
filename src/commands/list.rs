//! Commands for listing information about the validator or installed plugins.

use anyhow::{Context, Result};
use std::path::Path;
use std::process::ExitCode;

use super::{println_wrapped, println_wrapped_no_indent, TextWrapper};
use crate::index::PresetIndexResult;

// TODO: The indexing here always happens in the same process. We should move this over to out of
//       process scanning at some point.

/// Lists basic information about all installed CLAP plugins.
pub fn plugins(json: bool) -> Result<ExitCode> {
    let plugin_index = crate::index::index();

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&plugin_index).expect("Could not format JSON")
        );
    } else {
        let mut wrapper = TextWrapper::default();
        for (i, (plugin_path, metadata)) in plugin_index.0.into_iter().enumerate() {
            if i > 0 {
                println!();
            }

            println_wrapped!(
                wrapper,
                "{}: (CLAP {}.{}.{}, contains {} {})",
                plugin_path.display(),
                metadata.version.0,
                metadata.version.1,
                metadata.version.2,
                metadata.plugins.len(),
                if metadata.plugins.len() == 1 {
                    "plugin"
                } else {
                    "plugins"
                },
            );

            for plugin in metadata.plugins {
                println!();
                println_wrapped!(
                    wrapper,
                    " - {} {} ({})",
                    plugin.name,
                    plugin.version.as_deref().unwrap_or("(unknown version)"),
                    plugin.id
                );

                // Whether it makes sense to always show optional fields or not depends on
                // the field
                if let Some(description) = plugin.description {
                    println_wrapped_no_indent!(wrapper, "   {description}");
                }
                println!();
                println_wrapped!(
                    wrapper,
                    "   vendor: {}",
                    plugin.vendor.as_deref().unwrap_or("(unknown)")
                );
                if let Some(manual_url) = plugin.manual_url {
                    println_wrapped!(wrapper, "   manual url: {manual_url}");
                }
                if let Some(support_url) = plugin.support_url {
                    println_wrapped!(wrapper, "   support url: {support_url}");
                }
                println_wrapped!(wrapper, "   features: [{}]", plugin.features.join(", "));
            }
        }
    }

    Ok(ExitCode::SUCCESS)
}

/// Lists presets for one, more, or all plugins.
pub fn presets<P>(json: bool, plugin_paths: Option<&[P]>) -> Result<ExitCode>
where
    P: AsRef<Path>,
{
    // TODO: When preset flags are set omitted they should be inherited from the location. This
    //       currently does not happen.
    let preset_index = match plugin_paths {
        Some(plugin_paths) => crate::index::index_presets(plugin_paths, false),
        None => {
            let plugin_index = crate::index::index();
            let all_plugin_paths = plugin_index.0.keys();

            // This 'true' indicates that plugins that don't support the preset discovery mechanism
            // should be silently skipped
            crate::index::index_presets(all_plugin_paths, true)
        }
    }
    .context("Error while crawling presets")?;
    let has_errors = preset_index
        .0
        .values()
        .any(|result| matches!(result, PresetIndexResult::Error(_)));

    if !json {
        log::warn!(
            "Pretty printing has not yet been implemented for presets. I hope you like JSON."
        )
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&preset_index).expect("Could not format JSON")
    );

    Ok(if has_errors {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// Lists all available test cases.
pub fn tests(json: bool) -> Result<ExitCode> {
    let list = crate::tests::TestList::default();

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&list).expect("Could not format JSON")
        );
    } else {
        let mut wrapper = TextWrapper::default();

        println!("Plugin library tests:");
        for (test_name, test_description) in list.plugin_library_tests {
            println_wrapped!(wrapper, "- {test_name}: {test_description}");
        }

        println!("\nPlugin tests:");
        for (test_name, test_description) in list.plugin_tests {
            println_wrapped!(wrapper, "- {test_name}: {test_description}");
        }
    }

    Ok(ExitCode::SUCCESS)
}
