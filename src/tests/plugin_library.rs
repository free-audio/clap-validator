//! Tests for entire plugin libraries. These are mostly used to test plugin scanning behavior.

use clap::ValueEnum;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use super::{TestCase, TestResult};

mod factories;
mod scanning;

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
                "Checks whether the plugin can be scanned in under {} milliseconds.",
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
            PluginLibraryTestCase::ScanTime => scanning::test_scan_time(library_path),
            PluginLibraryTestCase::QueryNonexistentFactory => {
                factories::test_query_nonexistent_factory(library_path)
            }
            PluginLibraryTestCase::CreateIdWithTrailingGarbage => {
                factories::test_create_id_with_trailing_garbage(library_path)
            }
        };

        self.create_result(status)
    }
}
