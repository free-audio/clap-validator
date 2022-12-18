//! The base of the validation framework. This contains utilities for setting up a test case in a
//! way that somewhat mimics a real host.

use anyhow::{Context, Result};
use clap::{Args, ValueEnum};
use clap_sys::version::clap_version_is_compatible;
use regex::RegexBuilder;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use strum::IntoEnumIterator;

use crate::plugin::library::PluginLibrary;
use crate::tests::{PluginLibraryTestCase, PluginTestCase, TestCase, TestResult, TestStatus};
use crate::util;

/// The results of running the validation test suite on one or more plugins. Use the
/// [`tally()`][Self::tally()] method to compute the number of successful and failed tests.
///
/// Uses `BTreeMap`s purely so the order is stable.
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

/// Statistics for the validator.
pub struct ValidationTally {
    /// The number of passed test cases.
    pub num_passed: u32,
    /// The number of failed or crashed test cases.
    pub num_failed: u32,
    /// The number of skipped test cases.
    pub num_skipped: u32,
    /// The number of test cases resulting in a warning.
    pub num_warnings: u32,
}

/// Options for the validator.
#[derive(Debug, Args)]
pub struct ValidatorSettings {
    /// Paths to one or more plugins that should be validated.
    #[arg(required = true)]
    pub paths: Vec<PathBuf>,
    /// Only validate plugins with this ID.
    ///
    /// If the plugin library contains multiple plugins, then you can pass a single plugin's ID
    /// to this option to only validate that plugin. Otherwise all plugins in the library are
    /// validated.
    #[arg(short = 'i', long)]
    pub plugin_id: Option<String>,
    /// Print the test output as JSON instead of human readable text.
    #[arg(long)]
    pub json: bool,
    /// Only run the tests that match this case-insensitive regular expression.
    #[arg(short = 'f', long)]
    pub test_filter: Option<String>,
    /// Changes the behavior of -f/--test-filter to skip matching tests instead.
    #[arg(short = 'v', long)]
    pub invert_filter: bool,
    /// When running the validation out-of-process, hide the plugin's output.
    ///
    /// This can be useful for validating noisy plugins.
    #[arg(long)]
    pub hide_output: bool,
    /// Only show failed tests.
    ///
    /// This affects both the human readable and the JSON output.
    #[arg(long)]
    pub only_failed: bool,
    /// Run the tests within this process.
    ///
    /// Tests are normally run in separate processes in case the plugin crashes. Another benefit
    /// of the out-of-process validation is that the test always starts from a clean state.
    /// Using this option will remove those protections, but in turn the tests may run faster.
    #[arg(long)]
    pub in_process: bool,
}

/// Options for running a single test. This is used for the out-of-process testing method. This
/// option is hidden from the CLI as it's merely an implementation detail.
#[derive(Debug, Args)]
pub struct SingleTestSettings {
    pub test_type: SingleTestType,
    /// The path to the plugin's library.
    pub path: PathBuf,
    /// The ID of the plugin within the library that needs to be tested.
    pub plugin_id: String,
    /// The name of the test to run. [`TestCase`]s can be converted to and from strings to
    /// facilitate this.
    pub name: String,
    /// The name of the file to write the test's JSON result to. This is not done through STDIO
    /// because the hosted plugin may also write things there.
    #[arg(long)]
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

    // Before doing anything, we need to make sure any temporary artifact files from the previous
    // run are cleaned up. These are used for things like state dumps when one of the state tests
    // fail. This is allowed to fail since the directory may not exist and even if it does and we
    // cannot remove it, then that may not be a problem.
    let _ = std::fs::remove_dir_all(util::validator_temp_dir());
    let test_filter_re = settings
        .test_filter
        .as_deref()
        .map(|filter| {
            RegexBuilder::new(filter)
                .case_insensitive(true)
                .build()
                .context("The test filter is not a valid regular expression")
        })
        .transpose()?;

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
        for test in PluginLibraryTestCase::iter() {
            let test_name = test.to_string();
            match (&test_filter_re, settings.invert_filter) {
                (Some(test_filter_re), false) if !test_filter_re.is_match(&test_name) => continue,
                (Some(test_filter_re), true) if test_filter_re.is_match(&test_name) => continue,
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
            log::debug!(
                "'{}' uses an unsupported CLAP version ({}.{}.{}), skipping...",
                library_path.display(),
                metadata.version.0,
                metadata.version.1,
                metadata.version.2
            );

            continue;
        }

        // We only now know how many tests will be run for this plugin library
        for plugin_metadata in metadata.plugins {
            if results.plugin_tests.contains_key(&plugin_metadata.id) {
                anyhow::bail!(
                    "Duplicate plugin ID in validation results: '{}' ({}) has already been \
                     validated",
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
            for test in PluginTestCase::iter() {
                let test_name = test.to_string();
                match (&test_filter_re, settings.invert_filter) {
                    (Some(test_filter_re), false) if !test_filter_re.is_match(&test_name) => {
                        continue
                    }
                    (Some(test_filter_re), true) if test_filter_re.is_match(&test_name) => continue,
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
            let test_case = settings
                .name
                .parse::<PluginLibraryTestCase>()
                .with_context(|| format!("Unknown test name: {}", &settings.name))?;

            test_case.run_in_process(&settings.path)
        }
        SingleTestType::Plugin => {
            let plugin_library = PluginLibrary::load(&settings.path)
                .with_context(|| format!("Could not load '{}'", settings.path.display()))?;
            let test_case = settings
                .name
                .parse::<PluginTestCase>()
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

impl ValidationResult {
    /// Count the number of passing, failing, and skipped tests.
    pub fn tally(&self) -> ValidationTally {
        let mut num_passed = 0;
        let mut num_failed = 0;
        let mut num_skipped = 0;
        let mut num_warnings = 0;
        for test in self
            .plugin_library_tests
            .values()
            .chain(self.plugin_tests.values())
            .flatten()
        {
            match test.status {
                TestStatus::Success { .. } => num_passed += 1,
                TestStatus::Crashed { .. } | TestStatus::Failed { .. } => num_failed += 1,
                TestStatus::Skipped { .. } => num_skipped += 1,
                TestStatus::Warning { .. } => num_warnings += 1,
            }
        }

        ValidationTally {
            num_passed,
            num_failed,
            num_skipped,
            num_warnings,
        }
    }
}

impl ValidationTally {
    /// Get the total number of tests run.
    pub fn total(&self) -> u32 {
        self.num_passed + self.num_failed + self.num_skipped
    }
}
