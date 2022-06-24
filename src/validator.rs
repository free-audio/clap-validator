//! The base of the validation framework. This contains utilities for setting up a test case in a
//! way that somewhat mimics a real host.

use anyhow::Result;
use clap::Args;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

mod tests;

/// A test case for testing the behavior of a plugin. This `Test` object contains the result of a
/// test, which is serialized to and from JSON so the test can be run in another process.
#[derive(Debug, Deserialize, Serialize)]
pub struct Test {
    /// The name of this test.
    name: String,
    /// A description of what this test case has tested.
    description: String,
    /// The outcome of the test.
    result: TestResult,
}

/// The result of running a test. Skipped and failed test may optionally include an explanation for
/// why this happened.
#[derive(Debug, Deserialize, Serialize)]
pub enum TestResult {
    /// The test passed successfully.
    Success,
    /// The plugin segfaulted, SIGABRT'd, or otherwise crashed while running the test. This is only
    /// caught for out-of-process validation, for obvious reasons.
    Crashed { status: String },
    /// The test failed.
    Failed { reason: Option<String> },
    /// Preconditions for running the test were not met, so the test has been skipped.
    Skipped { reason: Option<String> },
}

/// A map indexed by plugin IDs containing the results of running the validation tests on one or
/// more plugins.
///
/// Uses a `BTreeMap` purely so the order is stable.
#[derive(Debug, Serialize)]
pub struct ValidationResult(pub BTreeMap<PathBuf, Vec<Test>>);

/// Options for the validator.
#[derive(Debug, Args)]
pub struct ValidatorSettings {
    /// Paths to one or more plugins that should be validated.
    #[clap(value_parser, required(true))]
    pub paths: Vec<PathBuf>,
    /// Only validate plugins with this ID.
    ///
    /// If the plugin library contains multiple plugins, then you can pass a single plugin's ID
    /// to this option to only validate that plugin. Otherwise all plugins in the library are
    /// validated.
    #[clap(value_parser, short = 'i', long)]
    pub plugin_id: Option<String>,
    /// Print the test output as JSON instead of human readable text.
    #[clap(value_parser, short, long)]
    pub json: bool,
    /// When running the validation out of process, hide the plugin's output.
    ///
    /// This can be useful for validating noisy plugins.
    #[clap(value_parser, long)]
    pub hide_output: bool,
    /// Run the tests within this process.
    ///
    /// Tests are normally run in separate processes in case the plugin crashes. Another benefit
    /// of the out of process validation is that the test always starts from a clean state.
    /// Using this option will remove those protections, but in turn the tests may run faster.
    #[clap(value_parser, long)]
    pub in_process: bool,
}

/// Run the validator using the specified settings. Returns an error if any of the plugin paths
/// could not loaded, or if the plugin ID filter did not match any plugins.
pub fn validate(settings: &ValidatorSettings) -> Result<ValidationResult> {
    let results: BTreeMap<PathBuf, Vec<Test>> = BTreeMap::new();

    // TODO: Dew it

    if let Some(plugin_id) = &settings.plugin_id {
        if results.is_empty() {
            anyhow::bail!("No plugins matched the plugin ID '{plugin_id}'");
        }
    }

    Ok(ValidationResult(results))
}
