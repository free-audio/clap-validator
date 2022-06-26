//! Tests for entire plugin libraries. These are mostly used to test plugin scanning behavior.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::Context;
use clap::ValueEnum;
use clap_sys::version::clap_version_is_compatible;

use crate::plugin::library::PluginLibrary;

use super::{TestCase, TestResult, TestStatus};

/// The string representation for [`PluginLibraryTestCase::ScanTime`].
const TEST_SCAN_TIME: &str = "scan-time";

const SCAN_TIME_LIMIT: Duration = Duration::from_millis(100);

/// Tests for entire CLAP libraries. These are mostly to ensure good plugin scanning practices. See
/// the module's heading for more information.
pub enum PluginLibraryTestCase {
    /// Asserts whether the plugin takes longer than `PLUGIN_SCAN_TIME_LIMIT` to scan.
    ScanTime,
}

impl<'a> TestCase<'a> for PluginLibraryTestCase {
    /// The path to a CLAP plugin library.
    type TestArgs = &'a Path;

    const ALL: &'static [Self] = &[PluginLibraryTestCase::ScanTime];

    fn from_str(string: &str) -> Option<Self> {
        match string {
            TEST_SCAN_TIME => Some(PluginLibraryTestCase::ScanTime),
            _ => None,
        }
    }

    fn as_str(&self) -> &'static str {
        match &self {
            PluginLibraryTestCase::ScanTime => TEST_SCAN_TIME,
        }
    }

    fn description(&self) -> String {
        match &self {
            PluginLibraryTestCase::ScanTime => format!(
                "Tests whether the plugin can be scanned in under {} milliseconds.",
                SCAN_TIME_LIMIT.as_millis()
            ),
        }
    }

    fn set_out_of_process_args(&self, command: &mut Command, library_path: Self::TestArgs) {
        let test_name = self.as_str();

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
        let result = match &self {
            PluginLibraryTestCase::ScanTime => {
                let test_start = Instant::now();

                {
                    // The library will be unloaded when this object is dropped, so that is part of
                    // the measurement
                    let plugin_library = PluginLibrary::load(library_path)
                        .with_context(|| format!("Could not load '{}'", library_path.display()));

                    // This goes through all plugins and builds a data structure containing
                    // information for all of those plugins, mimicing most of a DAW's plugin
                    // scanning process
                    let metadata = plugin_library.and_then(|plugin_library| {
                        plugin_library
                            .metadata()
                            .context("Could not query the plugin's metadata")
                    });

                    match metadata {
                        Ok(metadata) => {
                            if !clap_version_is_compatible(metadata.clap_version()) {
                                return self.create_result(TestStatus::Skipped {
                                    reason: Some(format!(
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
                                reason: Some(format!("{err:#}")),
                            })
                        }
                    }
                }

                let test_end = Instant::now();
                let init_duration = test_end - test_start;
                if init_duration <= SCAN_TIME_LIMIT {
                    let millis = init_duration.as_millis();
                    TestStatus::Success {
                        notes: Some(format!(
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
                        reason: Some(format!(
                            "The plugin took {} milliseconds to scan",
                            init_duration.as_millis()
                        )),
                    }
                }
            }
        };

        self.create_result(result)
    }
}
