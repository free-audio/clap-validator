//! The base of the validation framework. This contains utilities for setting up a test case in a
//! way that somewhat mimics a real host.

use crate::Verbosity;
use crate::cli::sandbox::{SandboxConfig, SandboxOperation};
use crate::cli::{Config, IteratorExt, panic_message};
use crate::commands::validate::ValidatorSettings;
use crate::plugin::library::PluginLibrary;
use crate::tests::{PluginInstanceTestCase, PluginLibraryTestCase, TestCase, TestGroup, TestResult, TestStatus};
use anyhow::{Context, Result};
use regex_lite::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use strum::IntoEnumIterator;

/// The results of running the validation test suite on one or more plugins.
#[derive(Default, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct ValidationResult {
    pub results: Vec<TestResult>,
}

/// Statistics for the validator.
pub struct ValidationTally {
    /// The number of passed test cases.
    pub num_passed: usize,
    /// The number of failed or crashed test cases.
    pub num_failed: usize,
    /// The number of skipped test cases.
    pub num_skipped: usize,
    /// The number of test cases resulting in a warning.
    pub num_warnings: usize,
}

impl ValidationResult {
    /// Count the number of passing, failing, and skipped tests.
    pub fn tally(&self) -> ValidationTally {
        let mut num_passed = 0;
        let mut num_failed = 0;
        let mut num_skipped = 0;
        let mut num_warnings = 0;

        for test in &self.results {
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

    pub fn group(&self) -> BTreeMap<TestGroup, Vec<TestResult>> {
        let mut groups: BTreeMap<TestGroup, Vec<TestResult>> = BTreeMap::new();

        for test in &self.results {
            groups.entry(test.test.group()).or_default().push(test.clone());
        }

        groups
    }

    /// Filter the test results using the specified filter function.
    pub fn filter(mut self, f: impl FnMut(&TestResult) -> bool) -> Self {
        self.results.retain(f);
        self
    }
}

impl ValidationTally {
    /// Get the total number of tests run.
    pub fn total(&self) -> usize {
        self.num_passed + self.num_failed + self.num_skipped + self.num_warnings
    }
}

/// Run the validator using the specified settings. Returns an error if any of the plugin paths
/// could not loaded, or if the plugin ID filter did not match any plugins.
pub fn validate(verbosity: Verbosity, settings: &ValidatorSettings, config: &Config) -> Result<ValidationResult> {
    let filter_test = {
        let test_filter_regexes = settings
            .include
            .iter()
            .map(|x| {
                Regex::new(x).with_context(|| format!("Could not parse the test filter regular expression '{}'", x))
            })
            .collect::<Result<Vec<_>>>()?;

        let test_exclude_regexes = settings
            .exclude
            .iter()
            .map(|x| {
                Regex::new(x).with_context(|| format!("Could not parse the test exclude regular expression '{}'", x))
            })
            .collect::<Result<Vec<_>>>()?;

        move |id: &str| {
            let config_enabled = config.is_test_enabled(id);
            let filter_matches = test_filter_regexes.is_empty() || test_filter_regexes.iter().any(|f| f.is_match(id));
            let exclude_matches = test_exclude_regexes.iter().any(|f| f.is_match(id));

            config_enabled && filter_matches && !exclude_matches
        }
    };

    let workers = match settings.jobs {
        _ if settings.in_process => Some(1),
        jobs => jobs,
    };

    // find all tests to run
    let tests = discover(&settings.paths, settings.plugin_id.as_deref(), filter_test)?;

    let mut results = tests
        .into_iter()
        .parallel_map(workers, |test| run_test(verbosity, settings, test))
        .collect::<Result<Vec<_>>>()?;

    results.sort_unstable_by(|a, b| a.test.cmp(&b.test));

    if results.is_empty() {
        anyhow::bail!("No tests selected to run");
    }

    Ok(ValidationResult { results })
}

/// Run a single test case with the specified settings.
fn run_test(verbosity: Verbosity, settings: &ValidatorSettings, test: TestCase) -> Result<TestResult> {
    let start = Instant::now();
    let validation = SandboxedValidation(test.clone());
    let (status, duration) = match settings.in_process {
        true => validation.run(),
        false => validation
            .run_sandboxed(SandboxConfig {
                hide_output: settings.hide_output,
                verbosity,
                timeout: Some(Duration::from_secs(45)),
            })
            .unwrap_or_else(|err| {
                (
                    TestStatus::Crashed {
                        details: err.to_string(),
                    },
                    start.elapsed(),
                )
            }),
    };

    match &status {
        TestStatus::Success { .. } => {
            log::info!("Test {} completed", test.name())
        }
        TestStatus::Warning { .. } => {
            log::warn!("Test {} completed with a warning", test.name())
        }
        TestStatus::Failed { .. } => {
            log::error!("Test {} failed", test.name())
        }
        TestStatus::Crashed { details } => {
            log::error!("Test {} crashed: {}", test.name(), details)
        }
        TestStatus::Skipped { .. } => {}
    }

    Ok(TestResult { test, duration, status })
}

/// Scan the plugins and construct a list of tests to run based on the specified paths, plugin ID filter, and test filter.
fn discover(paths: &[PathBuf], plugin_id: Option<&str>, filter_test: impl Fn(&str) -> bool) -> Result<Vec<TestCase>> {
    let mut result = Vec::new();

    for path in paths {
        let library = PluginLibrary::load(path)?;

        let metadata = library
            .metadata()
            .with_context(|| format!("Could not get the plugin metadata for library '{}'", path.display()))?;

        for test in PluginLibraryTestCase::iter() {
            if !filter_test(&test.to_string()) {
                continue;
            }

            result.push(TestCase::PluginLibrary {
                test,
                path: path.clone(),
            });
        }

        for plugin in metadata.plugins {
            if plugin_id.as_ref().is_none_or(|id| id == &plugin.id) {
                for test in PluginInstanceTestCase::iter() {
                    if !filter_test(&test.to_string()) {
                        continue;
                    }

                    result.push(TestCase::PluginInstance {
                        test,
                        path: path.clone(),
                        plugin_id: plugin.id.clone(),
                    });
                }
            }
        }
    }

    Ok(result)
}

#[derive(Serialize, Deserialize)]
pub struct SandboxedValidation(TestCase);

impl SandboxOperation for SandboxedValidation {
    const ID: &'static str = "validate";
    type Result = (TestStatus, Duration);

    fn run(&self) -> Self::Result {
        let start = Instant::now();

        let status = match catch_unwind(AssertUnwindSafe(|| self.0.run())) {
            Ok(status) => status,
            Err(panic) => TestStatus::Crashed {
                details: panic_message(&*panic),
            },
        };

        (status, start.elapsed())
    }
}
