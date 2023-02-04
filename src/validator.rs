//! The base of the validation framework. This contains utilities for setting up a test case in a
//! way that somewhat mimics a real host.

use anyhow::{Context, Result};
use clap::{Args, ValueEnum};
use clap_sys::version::clap_version_is_compatible;
use rayon::prelude::*;
use regex::{Regex, RegexBuilder};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use strum::IntoEnumIterator;

use crate::plugin::library::{PluginLibrary, PluginMetadata};
use crate::tests::{PluginLibraryTestCase, PluginTestCase, TestCase, TestResult, TestStatus};
use crate::util;

/// The results of running the validation test suite on one or more plugins. Use the
/// [`tally()`][Self::tally()] method to compute the number of successful and failed tests.
///
/// Uses `BTreeMap`s purely so the order is stable.
#[derive(Debug, Default, Serialize)]
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
    /// Don't run tests in parallel.
    ///
    /// This will cause the out-of-process tests to be run sequentially. Implied when the
    /// --in-process option is used. Can be useful for keeping plugin output in the correct order.
    #[arg(long, conflicts_with = "in_process")]
    pub no_parallel: bool,
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

    // The tests can optionally be run in parallel. This is not the default since some plugins may
    // not handle it correctly, event when the plugins are loaded in different processes. It's also
    // incompatible with the in-process mode.
    // NOTE: The parallel iterators don't preserve the iterator order, so to ensure consistency the
    //       results are sorted afterwards
    // TODO: There doesn't seem to be a way to run rayon iterators on the main thread, so the
    //       parallel and scalar versions need to be duplicated here. We could also create a single
    //       threaded shim that implements Rayon's parallel iterator methods, and then branch on the
    //       places where we create parallel iterators instead.
    let mut results = if settings.no_parallel || settings.in_process {
        settings
            .paths
            .iter()
            .map(|library_path| {
                // We distinguish between two separate classes of tests: tests for an entire plugin
                // library, and tests for a single plugin contained witin that library. The former
                // group of tests are run first and they only receive the path to the plugin library
                // as their argument, while the second class of tests receive an already loaded
                // plugin library and a plugin ID as their arugmetns. We'll start with the tests for
                // entire plugin libraries so the in-process mode makes a bit more sense. Otherwise
                // we would be measuring plugin scanning time on libraries that may still be loaded
                // in the process.
                let mut plugin_library_tests: BTreeMap<PathBuf, Vec<TestResult>> = BTreeMap::new();
                plugin_library_tests.insert(
                    library_path.clone(),
                    PluginLibraryTestCase::iter()
                        .filter(|test| test_filter(test, settings, &test_filter_re))
                        .map(|test| run_test(&test, settings, library_path))
                        .collect::<Result<Vec<TestResult>>>()?,
                );

                // And these are the per-plugin instance tests
                let plugin_library = PluginLibrary::load(library_path)
                    .with_context(|| format!("Could not load '{}'", library_path.display()))?;
                let plugin_metadata = plugin_library.metadata().with_context(|| {
                    format!(
                        "Could not fetch plugin metadata for '{}'",
                        library_path.display()
                    )
                })?;
                if !clap_version_is_compatible(plugin_metadata.clap_version()) {
                    log::debug!(
                        "'{}' uses an unsupported CLAP version ({}.{}.{}), skipping...",
                        library_path.display(),
                        plugin_metadata.version.0,
                        plugin_metadata.version.1,
                        plugin_metadata.version.2
                    );

                    // Since this is a map-reduce, this acts like a continue statement in a loop. We
                    // could use `.filter_map()` instead but that would only make things more
                    // complicated
                    return Ok(ValidationResult::default());
                }

                // We only now know how many tests will be run for this plugin library. We'll count
                // the number of plugins that match the filters and then compare that against the
                // number of entries in the map to make sure there are no dupli
                let plugin_tests: BTreeMap<String, Vec<TestResult>> = plugin_metadata
                    .plugins
                    .into_iter()
                    .filter(|plugin_metadata| plugin_filter(plugin_metadata, settings))
                    // We're building a `BTreeMap` containing the results for all plugins in the
                    // plugin's library
                    .map(|plugin_metadata| {
                        Ok((
                            plugin_metadata.id.clone(),
                            PluginTestCase::iter()
                                .filter(|test| test_filter(test, settings, &test_filter_re))
                                .map(|test| {
                                    run_test(
                                        &test,
                                        settings,
                                        (&plugin_library, &plugin_metadata.id),
                                    )
                                })
                                .collect::<Result<Vec<TestResult>>>()?,
                        ))
                    })
                    .collect::<Result<BTreeMap<_, _>>>()?;

                Ok(ValidationResult {
                    plugin_library_tests,
                    plugin_tests,
                })
            })
            .reduce(|a, b| {
                // Monads galore! The fact that we need to handle errors for plugin tests makes this
                // a bit more complicated.
                let (a, b) = (a?, b?);

                // In the serial version this could be done when iterating over the plugins, but
                // when using iterators you can't do that. But it's still essential to make sure we
                // don't test two versionsq of the same plugin.
                if a.intersects(&b) {
                    anyhow::bail!(
                        "Duplicate plugin ID in validation results. Maybe multiple versions of \
                         the same plugin are being validated."
                    );
                }

                Ok(ValidationResult::union(a, b))
            })
            .unwrap_or_else(|| Ok(ValidationResult::default()))
    } else {
        settings
            .paths
            .par_iter()
            .map(|library_path| {
                let mut plugin_library_tests: BTreeMap<PathBuf, Vec<TestResult>> = BTreeMap::new();
                plugin_library_tests.insert(
                    library_path.clone(),
                    PluginLibraryTestCase::iter()
                        .par_bridge()
                        .filter(|test| test_filter(test, settings, &test_filter_re))
                        .map(|test| run_test(&test, settings, library_path))
                        .collect::<Result<Vec<TestResult>>>()?,
                );

                let plugin_library = PluginLibrary::load(library_path)
                    .with_context(|| format!("Could not load '{}'", library_path.display()))?;
                let plugin_metadata = plugin_library.metadata().with_context(|| {
                    format!(
                        "Could not fetch plugin metadata for '{}'",
                        library_path.display()
                    )
                })?;
                if !clap_version_is_compatible(plugin_metadata.clap_version()) {
                    log::debug!(
                        "'{}' uses an unsupported CLAP version ({}.{}.{}), skipping...",
                        library_path.display(),
                        plugin_metadata.version.0,
                        plugin_metadata.version.1,
                        plugin_metadata.version.2
                    );

                    return Ok(ValidationResult::default());
                }

                let plugin_tests: BTreeMap<String, Vec<TestResult>> = plugin_metadata
                    .plugins
                    .into_par_iter()
                    .filter(|plugin_metadata| plugin_filter(plugin_metadata, settings))
                    .map(|plugin_metadata| {
                        Ok((
                            plugin_metadata.id.clone(),
                            PluginTestCase::iter()
                                .par_bridge()
                                .filter(|test| test_filter(test, settings, &test_filter_re))
                                .map(|test| {
                                    run_test(
                                        &test,
                                        settings,
                                        (&plugin_library, &plugin_metadata.id),
                                    )
                                })
                                .collect::<Result<Vec<TestResult>>>()?,
                        ))
                    })
                    .collect::<Result<BTreeMap<_, _>>>()?;

                Ok(ValidationResult {
                    plugin_library_tests,
                    plugin_tests,
                })
            })
            .reduce(
                || Ok(ValidationResult::default()),
                |a, b| {
                    let (a, b) = (a?, b?);

                    if a.intersects(&b) {
                        anyhow::bail!(
                            "Duplicate plugin ID in validation results. Maybe multiple versions \
                             of the same plugin are being validated."
                        );
                    }

                    Ok(ValidationResult::union(a, b))
                },
            )
    }?;

    // The parallel iterators don't preserve order, so this needs to be sorted to make sure the test
    // results are always reported in the same order
    for tests in results
        .plugin_tests
        .values_mut()
        .chain(results.plugin_library_tests.values_mut())
    {
        tests.sort_by(|a, b| Ord::cmp(&a.name, &b.name));
    }

    if let Some(plugin_id) = &settings.plugin_id {
        if results.plugin_tests.is_empty() {
            anyhow::bail!("No plugins matched the plugin ID '{plugin_id}'.");
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

/// The filter function for determining whether or not a test should be run based on the validator's
/// settings settings.
fn test_filter<'a, T: TestCase<'a>>(
    test: &T,
    settings: &ValidatorSettings,
    test_filter_re: &Option<Regex>,
) -> bool {
    let test_name = test.to_string();
    match (&test_filter_re, settings.invert_filter) {
        (Some(test_filter_re), false) if !test_filter_re.is_match(&test_name) => false,
        (Some(test_filter_re), true) if test_filter_re.is_match(&test_name) => false,
        _ => true,
    }
}

/// The filter function for determining whether or not tests should be run for a particular plugin.
fn plugin_filter(plugin_metadata: &PluginMetadata, settings: &ValidatorSettings) -> bool {
    // It's possible to filter by plugin ID in case you want to validate a single plugin
    // from a plugin library containing multiple plugins
    #[allow(clippy::match_like_matches_macro)]
    match &settings.plugin_id {
        Some(plugin_id) if &plugin_metadata.id != plugin_id => false,
        _ => true,
    }
}

/// The filter function for determining whether or not a test should be run based on the validator's
/// settings settings.
fn run_test<'a, T: TestCase<'a>>(
    test: &T,
    settings: &ValidatorSettings,
    args: T::TestArgs,
) -> Result<TestResult> {
    if settings.in_process {
        Ok(test.run_in_process(args))
    } else {
        test.run_out_of_process(args, settings.hide_output)
    }
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

    // Check whether the maps in the object intersect. Useful to ensure that a plugin ID only occurs
    // once in the outputs before merging them.
    pub fn intersects(&self, other: &Self) -> bool {
        for key in other.plugin_library_tests.keys() {
            if self.plugin_library_tests.contains_key(key) {
                return true;
            }
        }

        for key in other.plugin_tests.keys() {
            if self.plugin_tests.contains_key(key) {
                return true;
            }
        }

        false
    }

    /// Merge the results from two validation result objects. If `other` contains a key that also
    /// exists in this object, then the version from `other` is used.
    pub fn union(mut self, other: Self) -> Self {
        self.plugin_library_tests.extend(other.plugin_library_tests);
        self.plugin_tests.extend(other.plugin_tests);

        self
    }
}

impl ValidationTally {
    /// Get the total number of tests run.
    pub fn total(&self) -> u32 {
        self.num_passed + self.num_failed + self.num_skipped
    }
}
