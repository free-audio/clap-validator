//! The base of the validation framework. This contains utilities for setting up a test case in a
//! way that somewhat mimics a real host.

use anyhow::{Context, Result};
use clap::Args;
use clap_sys::version::clap_version_is_compatible;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use self::tests::{TestCase, TestResult};
use crate::plugin::library::ClapPluginLibrary;

mod tests;

/// A map indexed by plugin IDs containing the results of running the validation tests on one or
/// more plugins.
///
/// Uses a `BTreeMap` purely so the order is stable.
#[derive(Debug, Serialize)]
pub struct ValidationResult(pub BTreeMap<String, Vec<TestResult>>);

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
    /// Only run the tests that match this string.
    ///
    /// This is case-sensitive, and does not match regular expressions.
    #[clap(value_parser, short = 'f', long)]
    pub test_filter: Option<String>,
    /// When running the validation out-of-process, hide the plugin's output.
    ///
    /// This can be useful for validating noisy plugins.
    #[clap(value_parser, long)]
    pub hide_output: bool,
    /// Run the tests within this process.
    ///
    /// Tests are normally run in separate processes in case the plugin crashes. Another benefit
    /// of the out-of-process validation is that the test always starts from a clean state.
    /// Using this option will remove those protections, but in turn the tests may run faster.
    #[clap(value_parser, long)]
    pub in_process: bool,
}

/// Options for running a single test. This is used for the out-of-process testing method. This
/// option is hidden from the CLI as it's merely an implementation detail.
#[derive(Debug, Args)]
pub struct SingleTestSettings {
    /// The path to the plugin's library.
    #[clap(value_parser, required(true))]
    pub path: PathBuf,
    /// The ID of the plugin within the library that needs to be tested.
    #[clap(value_parser)]
    pub plugin_id: String,
    /// The name of the test to run. [`TestCase`]s can be converted to and from strings to
    /// facilitate this.
    #[clap(value_parser)]
    pub name: String,
    /// The name of the file to write the test's JSON result to. This is not done through STDIO
    /// because the hosted plugin may also write things there.
    #[clap(value_parser, long)]
    pub output_file: PathBuf,
}

/// Run the validator using the specified settings. Returns an error if any of the plugin paths
/// could not loaded, or if the plugin ID filter did not match any plugins.
pub fn validate(settings: &ValidatorSettings) -> Result<ValidationResult> {
    let mut results: BTreeMap<String, Vec<TestResult>> = BTreeMap::new();

    // TODO: We now gather all the results and print everything in one go at the end. This is the
    //       only way to do JSON, but for the human readable version printing things as we go could
    //       be nice.
    for library_path in &settings.paths {
        let plugin_library = ClapPluginLibrary::load(library_path)
            .with_context(|| format!("Could not load '{}'", library_path.display()))?;
        let metadata = plugin_library.metadata().with_context(|| {
            format!(
                "Could not fetch plugin metadata for '{}'",
                library_path.display()
            )
        })?;
        if !clap_version_is_compatible(metadata.clap_version()) {
            eprintln!(
                "'{}' uses an unsupported CLAP version ({}.{}.{}), skipping...",
                library_path.display(),
                metadata.version.0,
                metadata.version.1,
                metadata.version.2
            );

            continue;
        }

        for plugin_metadata in metadata.plugins {
            if results.contains_key(&plugin_metadata.id) {
                anyhow::bail!(
                    "Duplicate plugin ID in validation results: '{}' ({}) has already been validated",
                    plugin_metadata.id,
                    library_path.display(),
                );
            }

            // It's possible to filter by plugin ID in case you want to validate a single plugin
            // from a plugin library containing multiple plugins
            match &settings.plugin_id {
                Some(plugin_id) if &plugin_metadata.id != plugin_id => continue,
                _ => (),
            }

            let mut test_results = Vec::new();
            for test in TestCase::ALL {
                match &settings.test_filter {
                    Some(test_filter) if !test.as_str().contains(test_filter) => continue,
                    _ => (),
                }

                test_results.push(if settings.in_process {
                    test.run_in_process(&plugin_library, &plugin_metadata.id)
                } else {
                    test.run_out_of_process(
                        &plugin_library,
                        &plugin_metadata.id,
                        settings.hide_output,
                    )?
                });
            }

            results.insert(plugin_metadata.id, test_results);
        }
    }

    if let Some(plugin_id) = &settings.plugin_id {
        if results.is_empty() {
            anyhow::bail!("No plugins matched the plugin ID '{plugin_id}'");
        }
    }

    Ok(ValidationResult(results))
}

/// Run a single test case, and write the result to specified the output file path. This is used for
/// the out-of-process validation mode.
pub fn run_single_test(settings: &SingleTestSettings) -> Result<()> {
    let plugin_library = ClapPluginLibrary::load(&settings.path)
        .with_context(|| format!("Could not load '{}'", settings.path.display()))?;
    let test_case = TestCase::from_str(&settings.name)
        .with_context(|| format!("Unknown test name: {}", &settings.name))?;

    let result = test_case.run_in_process(&plugin_library, &settings.plugin_id);
    fs::write(
        &settings.output_file,
        serde_json::to_string(&result).context("Could not format the result as JSON")?,
    )
    .with_context(|| {
        format!(
            "Could not write the result to '{}'",
            settings.output_file.display()
        )
    })
}
