//! Tests for individual plugin instances.

use std::process::Command;

use clap::ValueEnum;

use super::{TestCase, TestResult, TestStatus};
use crate::plugin::library::ClapPluginLibrary;

/// The tests for individual CLAP plugins. See the module's heading for more information.
pub enum PluginTestCase {
    // TODO: Test some things
}

impl<'a> TestCase<'a> for PluginTestCase {
    /// A loaded CLAP plugin library and the ID of the plugin contained within that library that
    /// should be tested.
    type TestArgs = (&'a ClapPluginLibrary, &'a str);

    const ALL: &'static [Self] = &[];

    fn from_str(string: &str) -> Option<Self> {
        match string {
            _ => None,
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            // TODO: Add tests
            _ => "This enum doesn't have any variants right now",
        }
    }

    fn description(&self) -> String {
        match self {
            // TODO: Add tests
            _ => String::from("This enum doesn't have any variants right now"),
        }
    }

    fn set_out_of_process_args(&self, command: &mut Command, (library, plugin_id): Self::TestArgs) {
        let test_name = self.as_str();

        command
            .arg(
                crate::validator::SingleTestType::Plugin
                    .to_possible_value()
                    .unwrap()
                    .get_name(),
            )
            .arg(library.library_path())
            .arg(plugin_id)
            .arg(test_name);
    }

    fn run_in_process(&self, (library, plugin_id): Self::TestArgs) -> TestResult {
        let result = match &self {
            // TODO: Add tests
            _ => TestStatus::Skipped { reason: None },
        };

        TestResult {
            name: self.as_str().to_string(),
            description: self.description(),
            result,
        }
    }
}
