//! Commands for listing information about the validator or installed plugins.

use anyhow::{Context, Result};
use colored::Colorize;
use std::path::Path;
use std::process::ExitCode;

use super::{println_wrapped, println_wrapped_no_indent, TextWrapper};
use crate::index::PresetIndexResult;
use crate::plugin::preset_discovery::PresetFile;

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

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&preset_index).expect("Could not format JSON")
        );
    } else {
        let mut wrapper = TextWrapper::default();
        for (i, (plugin_path, result)) in preset_index.0.into_iter().enumerate() {
            if i > 0 {
                println!();
            }

            let provider_results = match result {
                PresetIndexResult::Success(provider_results) => provider_results,
                PresetIndexResult::Error(error) => {
                    println_wrapped!(wrapper, "{}:", plugin_path.display());
                    println!();
                    println_wrapped!(wrapper, "  {}: {}", "FAILED".red(), error);
                    continue;
                }
            };

            println_wrapped!(
                wrapper,
                "{}: (contains {} {})",
                plugin_path.display(),
                provider_results.len(),
                if provider_results.len() == 1 {
                    "preset provider"
                } else {
                    "preset providers"
                }
            );
            println!();

            for (i, provider_result) in provider_results.into_iter().enumerate() {
                if i > 0 {
                    println!();
                }

                println_wrapped!(
                    wrapper,
                    " - {} ({}) (contains {} {}, {} {}):",
                    provider_result.provider_name,
                    provider_result
                        .provider_vendor
                        .as_deref()
                        .unwrap_or("unknown vendor"),
                    provider_result.soundpacks.len(),
                    if provider_result.soundpacks.len() == 1 {
                        "soundpack"
                    } else {
                        "soundpacks"
                    },
                    provider_result.presets.len(),
                    if provider_result.presets.len() == 1 {
                        "preset"
                    } else {
                        "presets"
                    },
                );

                if !provider_result.soundpacks.is_empty() {
                    println!();
                    println!("   Soundpacks:");

                    for soundpack in provider_result.soundpacks {
                        println!();
                        println_wrapped!(wrapper, "   - {} ({})", soundpack.name, soundpack.id);
                        if let Some(description) = soundpack.description {
                            println_wrapped_no_indent!(wrapper, "     {}", description);
                        }
                        println!();
                        println_wrapped!(
                            wrapper,
                            "     vendor: {}",
                            soundpack.vendor.as_deref().unwrap_or("(unknown)")
                        );
                        if let Some(homepage_url) = soundpack.homepage_url {
                            println_wrapped!(wrapper, "     homepage url: {homepage_url}");
                        }
                        if let Some(image_uri) = soundpack.image_uri {
                            println_wrapped!(wrapper, "     image url: {image_uri}");
                        }
                        if let Some(release_timestamp) = soundpack.release_timestamp {
                            println_wrapped!(wrapper, "     released: {release_timestamp}");
                        }
                        println_wrapped!(wrapper, "     flags: {}", soundpack.flags);
                    }
                }

                if !provider_result.presets.is_empty() {
                    println!();
                    println!("   Presets URIs:");

                    for (preset_uri, preset_file) in provider_result.presets {
                        println!();
                        match preset_file {
                            PresetFile::Single(preset) => {
                                println_wrapped!(wrapper, "   - {}", preset_uri);

                                println!();
                                println_wrapped!(
                                    wrapper,
                                    "     {} ({})",
                                    preset.name,
                                    preset.plugin_ids_string()
                                );
                                if let Some(description) = preset.description {
                                    println_wrapped_no_indent!(wrapper, "     {}", description);
                                }
                                println!();
                                if !preset.creators.is_empty() {
                                    println_wrapped!(
                                        wrapper,
                                        "     {}: {}",
                                        if preset.creators.len() == 1 {
                                            "creator"
                                        } else {
                                            "creators"
                                        },
                                        preset.creators.join(", ")
                                    );
                                }
                                if let Some(soundpack_id) = preset.soundpack_id {
                                    println_wrapped!(wrapper, "     soundpack: {soundpack_id}");
                                }
                                if let Some(creation_time) = preset.creation_time {
                                    println_wrapped!(wrapper, "     created: {creation_time}");
                                }
                                if let Some(modification_time) = preset.modification_time {
                                    println_wrapped!(wrapper, "     modified: {modification_time}");
                                }
                                println_wrapped!(wrapper, "     flags: {}", preset.flags);
                                if !preset.features.is_empty() {
                                    println_wrapped!(
                                        wrapper,
                                        "     features: [{}]",
                                        preset.features.join(", ")
                                    );
                                }
                                if !preset.extra_info.is_empty() {
                                    println_wrapped!(
                                        wrapper,
                                        "     extra info: {:#?}",
                                        preset.extra_info
                                    );
                                }
                            }
                            PresetFile::Container(presets) => {
                                println_wrapped!(
                                    wrapper,
                                    "   - {} (contains {} {})",
                                    preset_uri,
                                    presets.len(),
                                    if presets.len() == 1 {
                                        "preset"
                                    } else {
                                        "presets"
                                    }
                                );

                                for (load_key, preset) in presets {
                                    println!();
                                    println_wrapped!(
                                        wrapper,
                                        "     - {} ({}, {})",
                                        preset.name,
                                        load_key,
                                        preset.plugin_ids_string()
                                    );
                                    if let Some(description) = preset.description {
                                        println_wrapped_no_indent!(
                                            wrapper,
                                            "       {}",
                                            description
                                        );
                                    }
                                    println!();
                                    if !preset.creators.is_empty() {
                                        println_wrapped!(
                                            wrapper,
                                            "       {}: {}",
                                            if preset.creators.len() == 1 {
                                                "creator"
                                            } else {
                                                "creators"
                                            },
                                            preset.creators.join(", ")
                                        );
                                    }
                                    if let Some(soundpack_id) = preset.soundpack_id {
                                        println_wrapped!(
                                            wrapper,
                                            "       soundpack: {soundpack_id}"
                                        );
                                    }
                                    if let Some(creation_time) = preset.creation_time {
                                        println_wrapped!(
                                            wrapper,
                                            "       created: {creation_time}"
                                        );
                                    }
                                    if let Some(modification_time) = preset.modification_time {
                                        println_wrapped!(
                                            wrapper,
                                            "       modified: {modification_time}"
                                        );
                                    }
                                    println_wrapped!(wrapper, "       flags: {}", preset.flags);
                                    if !preset.features.is_empty() {
                                        println_wrapped!(
                                            wrapper,
                                            "       features: [{}]",
                                            preset.features.join(", ")
                                        );
                                    }
                                    if !preset.extra_info.is_empty() {
                                        println_wrapped!(
                                            wrapper,
                                            "       extra info: {:#?}",
                                            preset.extra_info
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

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
