//! Tests for entire plugin libraries. These are mostly used to test plugin scanning behavior.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use clap::ValueEnum;

use super::{TestCase, TestResult, TestStatus};

/// The string representation for [`PluginLibraryTestCase::PluginScanTime`].
const TEST_PLUGIN_SCAN_TIME: &str = "plugin-scan-time";

const PLUGIN_SCAN_TIME_LIMIT: Duration = Duration::from_millis(100);

/// Tests for entire CLAP libraries. These are mostly to ensure good plugin scanning practices. See
/// the module's heading for more information.
pub enum PluginLibraryTestCase {
    /// Asserts whether the plugin takes longer than `PLUGIN_SCAN_TIME_LIMIT` to scan.
    PluginScanTime,
}

impl<'a> TestCase<'a> for PluginLibraryTestCase {
    /// The path to a CLAP plugin library.
    type TestArgs = &'a Path;

    const ALL: &'static [Self] = &[PluginLibraryTestCase::PluginScanTime];

    fn from_str(string: &str) -> Option<Self> {
        match string {
            TEST_PLUGIN_SCAN_TIME => Some(PluginLibraryTestCase::PluginScanTime),
            _ => None,
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            PluginLibraryTestCase::PluginScanTime => TEST_PLUGIN_SCAN_TIME,
        }
    }

    fn description(&self) -> String {
        match self {
            PluginLibraryTestCase::PluginScanTime => format!(
                "Tests whether the plugin can be scanned in under {} milliseconds.",
                PLUGIN_SCAN_TIME_LIMIT.as_millis()
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
            PluginLibraryTestCase::PluginScanTime => {
                let test_start = Instant::now();

                eprintln!("TODO: Actually implement the plugin scanning time");

                let test_end = Instant::now();
                let init_duration = test_end - test_start;
                if init_duration <= PLUGIN_SCAN_TIME_LIMIT {
                    TestStatus::Success
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

        TestResult {
            name: self.as_str().to_string(),
            description: self.description(),
            result,
        }
    }
}
