//! Tests for entire plugin libraries. These are mostly used to test plugin scanning behavior.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::Context;
use clap::ValueEnum;
use clap_sys::version::clap_version_is_compatible;

use crate::host::Host;
use crate::plugin::library::PluginLibrary;

use super::{TestCase, TestResult, TestStatus};

const SCAN_TIME_LIMIT: Duration = Duration::from_millis(100);

/// Tests for entire CLAP libraries. These are mostly to ensure good plugin scanning practices. See
/// the module's heading for more information, and the `description` function below for a
/// description of each test case.
#[derive(strum_macros::Display, strum_macros::EnumString, strum_macros::EnumIter)]
pub enum PluginLibraryTestCase {
    #[strum(serialize = "scan-time")]
    ScanTime,
    #[strum(serialize = "query-factory-nonexistent")]
    QueryNonexistentFactory,
    #[strum(serialize = "create-id-with-trailing-garbage")]
    CreateIdWithTrailingGarbage,
}

impl<'a> TestCase<'a> for PluginLibraryTestCase {
    /// The path to a CLAP plugin library.
    type TestArgs = &'a Path;

    fn description(&self) -> String {
        match self {
            PluginLibraryTestCase::ScanTime => format!(
                "Tests whether the plugin can be scanned in under {} milliseconds.",
                SCAN_TIME_LIMIT.as_millis()
            ),
            PluginLibraryTestCase::QueryNonexistentFactory => String::from(
                "Tries to query a factory from the plugin's entry point with a non-existent ID. \
                 This should return a null pointer.",
            ),
            PluginLibraryTestCase::CreateIdWithTrailingGarbage => String::from(
                "Attempts to create a plugin instance using an existing plugin ID with some extra \
                 text appended to the end. This should return a null pointer.",
            ),
        }
    }

    fn set_out_of_process_args(&self, command: &mut Command, library_path: Self::TestArgs) {
        let test_name = self.to_string();

        command
            .arg(
                crate::validator::SingleTestType::PluginLibrary
                    .to_possible_value()
                    .unwrap()
                    .get_name(),
            )
            .arg(library_path)
            // This is the plugin ID argument. We could make the `run-single-test` subcommand more
            // complicated and have this conditionally be required depending on the test type, but
            // this is simpler to reason about.
            .arg("(none)")
            .arg(test_name);
    }

    fn run_in_process(&self, library_path: Self::TestArgs) -> TestResult {
        let status = match self {
            PluginLibraryTestCase::ScanTime => {
                let test_start = Instant::now();

                {
                    // The library will be unloaded when this object is dropped, so that is part of
                    // the measurement
                    let library = PluginLibrary::load(library_path)
                        .with_context(|| format!("Could not load '{}'", library_path.display()));

                    // This goes through all plugins and builds a data structure containing
                    // information for all of those plugins, mimicing most of a DAW's plugin
                    // scanning process
                    let metadata = library.and_then(|plugin_library| {
                        plugin_library
                            .metadata()
                            .context("Could not query the plugin's metadata")
                    });

                    match metadata {
                        Ok(metadata) => {
                            if !clap_version_is_compatible(metadata.clap_version()) {
                                return self.create_result(TestStatus::Skipped {
                                    details: Some(format!(
                                        "'{}' uses an unsupported CLAP version ({}.{}.{})",
                                        library_path.display(),
                                        metadata.version.0,
                                        metadata.version.1,
                                        metadata.version.2
                                    )),
                                });
                            }
                        }
                        Err(err) => {
                            return self.create_result(TestStatus::Failed {
                                details: Some(format!("{err:#}")),
                            })
                        }
                    }
                }

                let test_end = Instant::now();
                let init_duration = test_end - test_start;
                if init_duration <= SCAN_TIME_LIMIT {
                    let millis = init_duration.as_millis();
                    TestStatus::Success {
                        details: Some(format!(
                            "The plugin can be scanned in {} {}.",
                            millis,
                            if millis == 1 {
                                "millisecond"
                            } else {
                                "milliseconds"
                            }
                        )),
                    }
                } else {
                    TestStatus::Failed {
                        details: Some(format!(
                            "The plugin took {} milliseconds to scan",
                            init_duration.as_millis()
                        )),
                    }
                }
            }
            PluginLibraryTestCase::QueryNonexistentFactory => {
                let library = PluginLibrary::load(library_path)
                    .with_context(|| format!("Could not load '{}'", library_path.display()));

                let status = library.and_then(|library| {
                    // This should be actually random instead of using a fixed seed like the other
                    // tests. This factory ID may not be used by anything.
                    let nonexistent_factory_id = format!("foo-factory-{}", rand::random::<u64>());
                    let nonexistent_factory_exists =
                        library.factory_exists(&nonexistent_factory_id);

                    // Since this factory doesn't exist, the plugin should always return a null
                    // pointer.
                    if nonexistent_factory_exists {
                        anyhow::bail!(
                            "Querying a factory with the non-existent factory ID \
                             '{nonexistent_factory_id} should return a null pointer, but the \
                             plugin returned a non-null pointer instead. The plugin may be \
                             unconditionally returning the plugin factory."
                        );
                    } else {
                        Ok(TestStatus::Success { details: None })
                    }
                });

                match status {
                    Ok(status) => status,
                    Err(err) => TestStatus::Failed {
                        details: Some(err.to_string()),
                    },
                }
            }
            PluginLibraryTestCase::CreateIdWithTrailingGarbage => {
                let library = PluginLibrary::load(library_path)
                    .with_context(|| format!("Could not load '{}'", library_path.display()));

                let status = library.and_then(|library| {
                    let metadata = library
                        .metadata()
                        .context("Could not query the plugin's metadata")?;
                    if !clap_version_is_compatible(metadata.clap_version()) {
                        return Ok(TestStatus::Skipped {
                            details: Some(format!(
                                "'{}' uses an unsupported CLAP version ({}.{}.{})",
                                library_path.display(),
                                metadata.version.0,
                                metadata.version.1,
                                metadata.version.2
                            )),
                        });
                    }

                    // We'll ask the plugin to create an instance of a plugin with the same ID as
                    // the first one from the factory, but with some additional data appended to the
                    // end. Since the plugin doesn't exist, this should return a null pointer.
                    let fake_plugin_id = match metadata.plugins.first() {
                        Some(descriptor) => {
                            // The x makes it cooler. And we'll try 100 versions in case the cooler
                            // verion of the plugin already exists.
                            let fake_plugin_id = (1..=100)
                                .map(|n| format!("{}x{n}", descriptor.id))
                                .find(|candidate| {
                                    !metadata.plugins.iter().any(|d| &d.id == candidate)
                                });

                            match fake_plugin_id {
                                Some(fake_plugin_id) => fake_plugin_id,
                                // This obviously should never be triggered unless someone is
                                // intentionally triggering it
                                None => {
                                    return Ok(TestStatus::Skipped {
                                        details: Some(String::from(
                                            "All of the coolest plugins already exists. In other \
                                             words, could not come up a fake unused plugin ID.",
                                        )),
                                    })
                                }
                            }
                        }
                        None => {
                            return Ok(TestStatus::Skipped {
                                details: Some(String::from(
                                    "The plugin library does not expose any plugins",
                                )),
                            })
                        }
                    };

                    // This should return an error/null-pointer instead of actually instantiating a
                    // plugin
                    match library.create_plugin(&fake_plugin_id, Host::new()) {
                        Ok(_) => anyhow::bail!(
                            "Creating a plugin instance with a non-existent plugin ID \
                             '{fake_plugin_id}' should return a null pointer, but it did not."
                        ),
                        Err(_) => Ok(TestStatus::Success { details: None }),
                    }
                });

                match status {
                    Ok(status) => status,
                    Err(err) => TestStatus::Failed {
                        details: Some(err.to_string()),
                    },
                }
            }
        };

        self.create_result(status)
    }
}
