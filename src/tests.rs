//! All tests in the validation test suite.
//!
//! Tests are split up in tests for the entire plugin library, and tests for individual plugins
//! within the library. The former group of tests exists mostly to ensure good plugin scanning
//! behavior.
//!
//! The results for the tests need to be serializable as JSON, and there also needs to be some way
//! to refer to a single test in a cli invocation (in order to be able to run tests out-of-process).
//! To facilitate this, the test cases are all identified by variants in an enum, and that enum can
//! be converted to and from a string representation.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

mod plugin_instance;
mod plugin_library;
pub mod rng;

pub use plugin_instance::PluginInstanceTestCase;
pub use plugin_library::PluginLibraryTestCase;

/// A description for a single test invocation. This contains all of the information necessary to run a single test.
#[derive(Deserialize, Serialize, Ord, PartialOrd, Eq, PartialEq, Clone)]
#[serde(rename_all = "kebab-case")]
pub enum TestCase {
    PluginLibrary {
        test: PluginLibraryTestCase,
        path: PathBuf,
    },

    PluginInstance {
        test: PluginInstanceTestCase,
        path: PathBuf,
        plugin_id: String,
    },
}

#[derive(Eq, PartialOrd, Ord, PartialEq)]
pub enum TestGroup {
    PluginLibrary(PathBuf),
    PluginInstance(PathBuf, String),
}

/// The result of running a test. Skipped and failed test may optionally include an explanation for
/// why this happened.
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
#[serde(tag = "code")]
pub enum TestStatus {
    /// The test passed successfully.
    Success { details: Option<String> },
    /// The plugin segfaulted, SIGABRT'd, or otherwise crashed while running the test. This is only
    /// caught for out-of-process validation, for obvious reasons.
    Crashed { details: String },
    /// The test failed.
    Failed { details: Option<String> },
    /// Preconditions for running the test were not met, so the test has been skipped.
    Skipped { details: Option<String> },
    /// The test did not succeed, but this should not be treated as a hard failure. This is reserved
    /// for tests involving runtime performance that might otherwise yield different results
    /// depending on the target system.
    Warning { details: Option<String> },
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct TestResult {
    pub test: TestCase,
    pub status: TestStatus,
    pub duration: Duration,
}

impl TestCase {
    pub fn name(&self) -> String {
        match self {
            Self::PluginLibrary { test, .. } => test.to_string(),
            Self::PluginInstance { test, .. } => test.to_string(),
        }
    }

    pub fn description(&self) -> String {
        match self {
            Self::PluginLibrary { test, .. } => test.description(),
            Self::PluginInstance { test, .. } => test.description(),
        }
    }

    pub fn group(&self) -> TestGroup {
        match self {
            Self::PluginLibrary { path, .. } => TestGroup::PluginLibrary(path.clone()),
            Self::PluginInstance { path, plugin_id, .. } => TestGroup::PluginInstance(path.clone(), plugin_id.clone()),
        }
    }

    pub fn run(&self) -> TestStatus {
        match self {
            Self::PluginLibrary { test, path } => test.run(path),
            Self::PluginInstance { test, path, plugin_id } => test.run(path, plugin_id),
        }
        .unwrap_or_else(|err| {
            let err = err.chain().map(|x| x.to_string()).collect::<Vec<_>>().join("\n");
            TestStatus::Failed { details: Some(err) }
        })
    }
}

impl TestStatus {
    /// Returns `true` if tests with this status should be shown when running the validator with the
    /// `--only-failed` option.
    pub fn failed_or_warning(&self) -> bool {
        match self {
            TestStatus::Success { .. } | TestStatus::Skipped { .. } => false,
            TestStatus::Warning { .. } | TestStatus::Crashed { .. } | TestStatus::Failed { .. } => true,
        }
    }

    /// Get the textual explanation for the test status, if this is available.
    pub fn details(&self) -> Option<&str> {
        match self {
            TestStatus::Success { details }
            | TestStatus::Failed { details }
            | TestStatus::Skipped { details }
            | TestStatus::Warning { details } => details.as_deref(),
            TestStatus::Crashed { details } => Some(details),
        }
    }
}

pub fn temporary_file(test_name: &str, plugin_id: &str, name: &str) -> anyhow::Result<(PathBuf, std::fs::File)> {
    let path = crate::cli::validator_temp_dir()
        .join(plugin_id)
        .join(test_name)
        .join(name);

    if path.exists() {
        panic!(
            "Tried to create a temporary file at '{}', but this file already exists",
            path.display()
        )
    }

    std::fs::create_dir_all(path.parent().unwrap())
        .expect("Could not create the directory for the test's temporary files");
    let file = std::fs::File::create(&path).expect("Could not create a temporary file for the test");

    Ok((path, file))
}
