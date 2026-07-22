//! Commands for listing information about the validator or installed plugins.

use crate::Verbosity;
use crate::cli::IteratorExt;
use crate::cli::sandbox::{SandboxConfig, SandboxOperation};
use crate::plugin::index::{SandboxedScanLibrary, ScanStatus, index_plugins};
use anyhow::{Context, Result};
use clap::Subcommand;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;
use strum::IntoEnumIterator;

/// Commands for listing tests and data realted to the installed plugins.
#[derive(Subcommand)]
pub enum ListCommand {
    /// Lists basic information about all installed CLAP plugins.
    Plugins {
        /// Print JSON instead of a human readable format.
        #[arg(short, long)]
        json: bool,
        /// Run the plugin indexing in-process instead of out-of-process.
        #[arg(long)]
        in_process: bool,
        /// When running the scans out-of-process, hide the plugin's output.
        #[arg(long, conflicts_with = "in_process")]
        hide_output: bool,
        /// Paths to one or more plugins that should be loaded and scanned, optional.
        ///
        /// All installed plugins are crawled if this value is missing.
        paths: Option<Vec<PathBuf>>,
    },
    /// Lists the available presets for one, more, or all installed CLAP plugins.
    Presets {
        /// Print JSON instead of a human readable format.
        #[arg(short, long)]
        json: bool,
        /// Run the plugin indexing in-process instead of out-of-process.
        #[arg(long)]
        in_process: bool,
        /// When running the scans out-of-process, hide the plugin's output.
        #[arg(long, conflicts_with = "in_process")]
        hide_output: bool,
        /// Paths to one or more plugins that should be indexed for presets, optional.
        ///
        /// All installed plugins are crawled if this value is missing.
        paths: Option<Vec<PathBuf>>,
        /// Limit the number of presets printed per plugin. Only applies to the human readable output.
        #[arg(short, long, conflicts_with = "json")]
        limit: Option<usize>,
    },

    /// Lists all available test cases.
    Tests {
        /// Print JSON instead of a human readable format.
        #[arg(short, long)]
        json: bool,
    },
}

pub fn list(verbosity: Verbosity, command: ListCommand) -> Result<ExitCode> {
    match command {
        ListCommand::Tests { json } => list_tests(json),
        ListCommand::Plugins {
            json,
            in_process,
            hide_output,
            paths,
        } => list_plugins(json, in_process, hide_output, verbosity, paths),
        ListCommand::Presets {
            json,
            in_process,
            hide_output,
            paths,
            limit,
        } => list_presets(
            json,
            in_process,
            hide_output,
            verbosity,
            paths,
            limit.unwrap_or(usize::MAX),
        ),
    }
}

/// List presets for one, more, or all installed CLAP plugins.
fn list_presets(
    json: bool,
    in_process: bool,
    hide_output: bool,
    verbosity: Verbosity,
    paths: Option<Vec<PathBuf>>,
    preset_limit: usize,
) -> Result<ExitCode> {
    let plugins = match paths {
        Some(paths) => paths,
        None => index_plugins().context("Error while crawling plugins")?,
    };

    let results = plugins
        .into_iter()
        .parallel_map(in_process.then_some(1), |path| {
            scan_single(path, in_process, true, hide_output, verbosity)
        })
        .collect::<Result<BTreeMap<_, _>>>()?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&results).expect("Could not format JSON")
        );
    } else {
        pretty::print_presets(results, preset_limit);
    }

    Ok(ExitCode::SUCCESS)
}

/// Lists basic information about all installed CLAP plugins.
fn list_plugins(
    json: bool,
    in_process: bool,
    hide_output: bool,
    verbosity: Verbosity,
    paths: Option<Vec<PathBuf>>,
) -> Result<ExitCode> {
    let plugins = match paths {
        Some(paths) => paths,
        None => index_plugins().context("Error while crawling plugins")?,
    };

    let results = plugins
        .into_iter()
        .parallel_map(in_process.then_some(1), |path| {
            scan_single(path, in_process, false, hide_output, verbosity)
        })
        .collect::<Result<BTreeMap<_, _>>>()?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&results).expect("Could not format JSON")
        );
    } else {
        pretty::print_plugins(results);
    }

    Ok(ExitCode::SUCCESS)
}

/// Lists all available test cases.
fn list_tests(json: bool) -> Result<ExitCode> {
    #[derive(Serialize)]
    #[serde(tag = "type", rename_all = "kebab-case")]
    enum TestJson {
        PluginLibrary { name: String, description: String },
        PluginInstance { name: String, description: String },
    }

    if json {
        let mut list = Vec::new();

        for test in crate::tests::PluginLibraryTestCase::iter() {
            list.push(TestJson::PluginLibrary {
                name: test.to_string(),
                description: test.description().to_string(),
            });
        }

        for test in crate::tests::PluginInstanceTestCase::iter() {
            list.push(TestJson::PluginInstance {
                name: test.to_string(),
                description: test.description().to_string(),
            });
        }

        println!(
            "{}",
            serde_json::to_string_pretty(&list).expect("Could not format JSON")
        );
    } else {
        let config = crate::cli::Config::from_current()?;
        pretty::print_tests(&config);
    }

    Ok(ExitCode::SUCCESS)
}

fn scan_single(
    path: PathBuf,
    in_process: bool,
    scan_presets: bool,
    hide_output: bool,
    verbosity: Verbosity,
) -> Result<(PathBuf, ScanStatus)> {
    let request = SandboxedScanLibrary {
        library_path: path.clone(),
        scan_presets,
    };

    let result = match in_process {
        true => request.run(),
        false => request
            .run_sandboxed(SandboxConfig {
                verbosity,
                hide_output,
                timeout: Some(std::time::Duration::from_secs(10)),
            })
            .unwrap_or_else(|err| ScanStatus::Crashed {
                details: err.to_string(),
            }),
    };

    Ok((path, result))
}

mod pretty {
    use crate::cli::{Config, Report, ReportItem, pluralize};
    use crate::plugin::index::ScanStatus;
    use crate::plugin::preset_discovery::*;
    use crate::tests::{PluginInstanceTestCase, PluginLibraryTestCase};
    use std::path::PathBuf;
    use strum::IntoEnumIterator;
    use yansi::Paint;

    pub fn print_tests(config: &Config) {
        let report_test = |name: &str, description: &str| {
            let mut report = Report {
                header: name.to_string(),
                items: vec![ReportItem::Text(description.to_string())],
                footer: vec![],
            };

            if !config.is_test_enabled(name) {
                report.footer.push("disabled".dim().italic().to_string());
            }

            report
        };

        let plugin_library_tests = PluginLibraryTestCase::iter().collect::<Vec<_>>();
        let plugin_instance_tests = PluginInstanceTestCase::iter().collect::<Vec<_>>();

        let mut library = Report {
            header: "Plugin Library".to_string(),
            items: vec![ReportItem::Text(
                "Tests for plugin factories, preset providers and plugin libraries (files) in general".to_string(),
            )],
            footer: vec![pluralize(plugin_library_tests.len(), "test")],
        };

        let mut plugin = Report {
            header: "Plugin".to_string(),
            items: vec![ReportItem::Text(
                "Tests for specific plugins within libraries, including their behavior during initialization, \
                 deinitialization, audio processing and callback handling."
                    .to_string(),
            )],
            footer: vec![pluralize(plugin_instance_tests.len(), "test")],
        };

        for test in &plugin_library_tests {
            library
                .items
                .push(ReportItem::Child(report_test(&test.to_string(), &test.description())));
        }

        for test in &plugin_instance_tests {
            plugin
                .items
                .push(ReportItem::Child(report_test(&test.to_string(), &test.description())));
        }

        println!("\n{}", library);
        println!("\n{}", plugin);
    }

    pub fn print_plugins(results: impl IntoIterator<Item = (PathBuf, ScanStatus)>) {
        let mut num_errors = 0;
        let mut num_files = 0;
        let mut num_plugins = 0;

        for (plugin_path, status) in results.into_iter() {
            // add to the tally
            num_files += 1;

            // handle and print errors if necessary
            let (library, duration) = match status {
                ScanStatus::Error { details } => {
                    num_errors += 1;

                    let report = Report {
                        header: plugin_path.display().to_string(),
                        items: vec![ReportItem::Text(details)],
                        footer: vec!["ERROR".red().to_string()],
                    };

                    println!("\n{}", report);
                    continue;
                }

                ScanStatus::Crashed { details } => {
                    num_errors += 1;

                    let report = Report {
                        header: plugin_path.display().to_string(),
                        items: vec![ReportItem::Text(details)],
                        footer: vec!["CRASHED".red().bold().to_string()],
                    };

                    println!("\n{}", report);
                    continue;
                }

                ScanStatus::Success { library, duration } => (library, duration),
            };

            // plugin library info
            let mut group = Report {
                header: plugin_path.display().to_string(),

                items: vec![ReportItem::Text(format!(
                    "CLAP {}.{}.{}",
                    library.version.0, library.version.1, library.version.2
                ))],

                footer: vec![
                    "OK".green().to_string(),
                    pluralize(library.plugins.len(), "plugin"),
                    format!("{}ms", duration.as_millis()).dim().to_string(),
                ],
            };

            // per plugin info
            for plugin in library.plugins {
                num_plugins += 1;

                let mut report = Report {
                    header: plugin.id,
                    ..Default::default()
                };

                report.items.push(ReportItem::Text(format!(
                    "{} {} ({})",
                    plugin.name,
                    plugin.version.as_deref().unwrap_or("(unknown version)"),
                    plugin.vendor.as_deref().unwrap_or("unknown vendor"),
                )));

                if let Some(description) = plugin.description {
                    report.items.push(ReportItem::Text(description));
                }

                let mut metadata = vec![];

                if let Some(url) = plugin.url {
                    metadata.push(("url".to_string(), url));
                }

                if let Some(manual_url) = plugin.manual_url {
                    metadata.push(("manual url".to_string(), manual_url));
                }

                if let Some(support_url) = plugin.support_url {
                    metadata.push(("support url".to_string(), support_url));
                }

                if !plugin.features.is_empty() {
                    metadata.push(("features".to_string(), plugin.features.join(" ")));
                }

                report.items.push(ReportItem::Table(metadata));
                group.items.push(ReportItem::Child(report));
            }

            println!("\n{}", group);
        }

        println!(
            "{}, {}, {}",
            pluralize(num_files, "file"),
            pluralize(num_plugins, "plugin"),
            pluralize(num_errors, "error")
        )
    }

    pub fn print_presets(results: impl IntoIterator<Item = (PathBuf, ScanStatus)>, preset_limit: usize) {
        fn report_preset(preset: &Preset, key: &str, location: &str) -> Report {
            let mut metadata = vec![];

            if !location.is_empty() {
                metadata.push(("location".to_string(), location.to_string()));
            }

            if !key.is_empty() {
                metadata.push(("key".to_string(), key.to_string()));
            }

            if !preset.plugin_ids.is_empty() {
                metadata.push((
                    "plugins".to_string(),
                    preset
                        .plugin_ids
                        .iter()
                        .map(|id| match &id.abi {
                            PluginAbi::Clap => id.id.clone(),
                            PluginAbi::Other(abi) => format!("{}:{}", abi, id.id),
                        })
                        .collect::<Vec<_>>()
                        .join(" "),
                ));
            }

            if let Some(soundpack_id) = &preset.soundpack_id {
                metadata.push(("soundpack".to_string(), soundpack_id.clone()));
            }

            if let Some(creation_time) = preset.creation_time {
                metadata.push(("created".to_string(), creation_time.to_string()));
            }

            if let Some(modification_time) = preset.modification_time {
                metadata.push(("modified".to_string(), modification_time.to_string()));
            }

            if !preset.creators.is_empty() {
                metadata.push(("creators".to_string(), preset.creators.join("; ")));
            }

            if !preset.features.is_empty() {
                metadata.push(("features".to_string(), preset.features.join("; ")));
            }

            for (key, value) in &preset.extra_info {
                metadata.push((key.clone(), value.clone()));
            }

            let flags = {
                let mut flags = vec![];
                if preset.flags.is_inherited {
                    flags.push("inherited");
                }
                if preset.flags.flags.is_favorite {
                    flags.push("favorite");
                }
                if preset.flags.flags.is_factory_content {
                    flags.push("factory");
                }
                if preset.flags.flags.is_demo_content {
                    flags.push("demo");
                }
                if preset.flags.flags.is_user_content {
                    flags.push("user");
                }
                flags.join(" ")
            };

            if !flags.is_empty() {
                metadata.push(("flags".to_string(), flags));
            }

            let mut report = Report {
                header: "Preset".to_string(),
                items: vec![],
                footer: vec![],
            };

            report.items.push(ReportItem::Text(preset.name.to_string()));

            if let Some(description) = &preset.description {
                report.items.push(ReportItem::Text(description.to_string()));
            }

            if !metadata.is_empty() {
                report.items.push(ReportItem::Table(metadata));
            }

            report
        }

        fn report_soundpack(soundpack: &Soundpack) -> Report {
            let mut report = Report {
                header: "Soundpack".to_string(),
                ..Default::default()
            };

            report.items.push(ReportItem::Text(format!(
                "{} ({})",
                soundpack.name,
                soundpack.vendor.as_deref().unwrap_or("unknown vendor")
            )));

            if let Some(description) = &soundpack.description {
                report.items.push(ReportItem::Text(description.to_string()));
            }

            let mut metadata = vec![];

            metadata.push(("id".to_string(), soundpack.id.clone()));

            if let Some(image_path) = &soundpack.image_path {
                metadata.push(("image".to_string(), image_path.to_string()));
            }

            if let Some(homepage_url) = &soundpack.homepage_url {
                metadata.push(("homepage".to_string(), homepage_url.clone()));
            }

            if let Some(release_timestamp) = soundpack.release_timestamp {
                metadata.push(("released".to_string(), release_timestamp.to_string()));
            }

            let flags = {
                let mut flags = vec![];
                if soundpack.flags.is_favorite {
                    flags.push("favorite");
                }
                if soundpack.flags.is_factory_content {
                    flags.push("factory");
                }
                if soundpack.flags.is_demo_content {
                    flags.push("demo");
                }
                if soundpack.flags.is_user_content {
                    flags.push("user");
                }
                flags.join(" ")
            };

            if !flags.is_empty() {
                metadata.push(("flags".to_string(), flags));
            }

            report
        }

        for (plugin_path, status) in results.into_iter() {
            let (library, duration) = match status {
                ScanStatus::Error { details } => {
                    let report = Report {
                        header: plugin_path.display().to_string(),
                        items: vec![ReportItem::Text(details)],
                        footer: vec!["ERROR".red().to_string()],
                    };

                    println!("\n{}", report);
                    continue;
                }

                ScanStatus::Crashed { details } => {
                    let report = Report {
                        header: plugin_path.display().to_string(),
                        items: vec![ReportItem::Text(details)],
                        footer: vec!["CRASHED".red().bold().to_string()],
                    };

                    println!("\n{}", report);
                    continue;
                }

                ScanStatus::Success { library, duration } => {
                    if library.preset_providers.is_empty() {
                        continue;
                    }

                    (library, duration)
                }
            };

            let mut group = Report {
                header: plugin_path.display().to_string(),

                items: vec![ReportItem::Text(format!(
                    "CLAP {}.{}.{}",
                    library.version.0, library.version.1, library.version.2
                ))],

                footer: vec![
                    "OK".green().to_string(),
                    pluralize(library.preset_providers.len(), "preset provider"),
                    format!("{}ms", duration.as_millis()).dim().to_string(),
                ],
            };

            for provider in library.preset_providers {
                let mut report = Report {
                    header: provider.provider_id,
                    footer: vec![
                        pluralize(provider.soundpacks.len(), "soundpack"),
                        pluralize(provider.presets.len(), "preset"),
                    ],
                    ..Default::default()
                };

                report.items.push(ReportItem::Text(format!(
                    "{} {}.{}.{} ({})",
                    provider.provider_name,
                    provider.provider_version.0,
                    provider.provider_version.1,
                    provider.provider_version.2,
                    provider.provider_vendor.as_deref().unwrap_or("unknown vendor"),
                )));

                for (index, soundpack) in provider.soundpacks.iter().enumerate() {
                    if index >= preset_limit {
                        report.items.push(ReportItem::Text(
                            format!("... and {} more soundpacks", provider.soundpacks.len() - preset_limit)
                                .dim()
                                .italic()
                                .to_string(),
                        ));

                        break;
                    }

                    report.items.push(ReportItem::Child(report_soundpack(soundpack)));
                }

                for (index, (location, preset)) in provider.presets.iter().enumerate() {
                    if index >= preset_limit {
                        report.items.push(ReportItem::Text(
                            format!("... and {} more presets", provider.presets.len() - preset_limit)
                                .dim()
                                .italic()
                                .to_string(),
                        ));

                        break;
                    }

                    match preset {
                        PresetFile::Single(preset) => {
                            report
                                .items
                                .push(ReportItem::Child(report_preset(preset, "", &location.to_string())));
                        }

                        PresetFile::Container(presets) => {
                            let mut container = Report {
                                header: "Preset Container".to_string(),
                                items: vec![],
                                footer: vec![],
                            };

                            container.items.push(ReportItem::Text(location.to_string()));

                            for (key, preset) in presets {
                                container.items.push(ReportItem::Child(report_preset(preset, key, "")));
                            }

                            container.footer.push(pluralize(presets.len(), "preset"));
                            report.items.push(ReportItem::Child(container));
                        }
                    }
                }

                group.items.push(ReportItem::Child(report));
            }

            println!("\n{}", group);
        }
    }
}
