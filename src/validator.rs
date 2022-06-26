//! The base of the validation framework. This contains utilities for setting up a test case in a
//! way that somewhat mimics a real host.

use anyhow::{Context, Result};
use clap::{Args, ValueEnum};
use clap_sys::version::clap_version_is_compatible;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use crate::plugin::library::PluginLibrary;
use crate::tests::{PluginLibraryTestCase, PluginTestCase, TestCase, TestResult};

/// The results of running the validation test suite on one or more plugins.
///
/// Uses a `BTreeMap`s purely so the order is stable.
#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct ValidationResult {
    /// A map indexed by plugin library paths containing the results of running the per-plugin
    /// library tests on one or more plugin libraries. These tests mainly examine the plugin's
    /// scanning behavior.
    pub plugin_library_tests: BTreeMap<PathBuf, Vec<TestResult>>,
    /// A map indexed by plugin IDs containing the results of running the per-plugin tests on one or
    /// more plugins.
    pub plugin_tests: BTreeMap<String, Vec<TestResult>>,
}

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
    #[clap(value_parser)]
    pub test_type: SingleTestType,
    /// The path to the plugin's library.
    #[clap(value_parser)]
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

/// The type of test to run when only running a single test. This is only used for out-of-process
/// validation.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum SingleTestType {
    /// A test for an entire plugin library.
    ///
    /// Used for testing scanning behavior.
    PluginLibrary,
    /// A test for an individual plugin instance.
    Plugin,
}

/// Run the validator using the specified settings. Returns an error if any of the plugin paths
/// could not loaded, or if the plugin ID filter did not match any plugins.
pub fn validate(settings: &ValidatorSettings) -> Result<ValidationResult> {
    let mut results = ValidationResult {
        plugin_library_tests: BTreeMap::new(),
        plugin_tests: BTreeMap::new(),
    };

    // TODO: We now gather all the results and print everything in one go at the end. This is the
    //       only way to do JSON, but for the human readable version printing things as we go could
    //       be nice.
    for library_path in &settings.paths {
        // We distinguish between two separate classes of tests: tests for an entire plugin library,
        // and tests for a single plugin contained witin that library. The former group of tests are
        // run first and they only receive the path to the plugin library as their argument, while
        // the second class of tests receive an already loaded plugin library and a plugin ID as
        // their arugmetns. We'll start with the tests for entire plugin libraries so the in-process
        // mode makes a bit more sense. Otherwise we would be measuring plugin scanning time on
        // libraries that may still be loaded in the process.
        let mut plugin_library_results = Vec::new();
        for test in PluginLibraryTestCase::ALL {
            match &settings.test_filter {
                Some(test_filter) if !test.as_str().contains(test_filter) => continue,
                _ => (),
            }

            plugin_library_results.push(if settings.in_process {
                test.run_in_process(library_path)
            } else {
                test.run_out_of_process(library_path, settings.hide_output)?
            });
        }

        results
            .plugin_library_tests
            .insert(PathBuf::from(&library_path), plugin_library_results);

        // And these are the per-plugin instance tests
        let plugin_library = PluginLibrary::load(library_path)
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
            if results.plugin_tests.contains_key(&plugin_metadata.id) {
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

            let mut plugin_test_results = Vec::new();
            for test in PluginTestCase::ALL {
                match &settings.test_filter {
                    Some(test_filter) if !test.as_str().contains(test_filter) => continue,
                    _ => (),
                }

                plugin_test_results.push(if settings.in_process {
                    test.run_in_process((&plugin_library, &plugin_metadata.id))
                } else {
                    test.run_out_of_process(
                        (&plugin_library, &plugin_metadata.id),
                        settings.hide_output,
                    )?
                });
            }

            results
                .plugin_tests
                .insert(plugin_metadata.id, plugin_test_results);
        }
    }

    if let Some(plugin_id) = &settings.plugin_id {
        if results.plugin_tests.is_empty() {
            anyhow::bail!("No plugins matched the plugin ID '{plugin_id}'");
        }
    }

    Ok(results)
}

/// Run a single test case, and write the result to specified the output file path. This is used for
/// the out-of-process validation mode.
pub fn run_single_test(settings: &SingleTestSettings) -> Result<()> {
    let result = match settings.test_type {
        SingleTestType::PluginLibrary => {
            let test_case = PluginLibraryTestCase::from_str(&settings.name)
                .with_context(|| format!("Unknown test name: {}", &settings.name))?;

            test_case.run_in_process(&settings.path)
        }
        SingleTestType::Plugin => {
            let plugin_library = PluginLibrary::load(&settings.path)
                .with_context(|| format!("Could not load '{}'", settings.path.display()))?;
            let test_case = PluginTestCase::from_str(&settings.name)
                .with_context(|| format!("Unknown test name: {}", &settings.name))?;

            test_case.run_in_process((&plugin_library, &settings.plugin_id))
        }
    };

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
