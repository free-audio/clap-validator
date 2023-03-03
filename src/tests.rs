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

use anyhow::{Context, Result};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fmt::Display;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::str::FromStr;
use strum::IntoEnumIterator;

use crate::{util, Verbosity};

mod plugin;
mod plugin_library;
pub mod rng;

pub use plugin::PluginTestCase;
pub use plugin_library::PluginLibraryTestCase;

/// A test case for testing the behavior of a plugin. This `Test` object contains the result of a
/// test, which is serialized to and from JSON so the test can be run in another process.
#[derive(Debug, Deserialize, Serialize)]
pub struct TestResult {
    /// The name of this test.
    pub name: String,
    /// A description of what this test case has tested.
    pub description: String,
    /// The outcome of the test.
    pub status: TestStatus,
}

/// The result of running a test. Skipped and failed test may optionally include an explanation for
/// why this happened.
#[derive(Debug, Deserialize, Serialize)]
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

/// Stores all of the available tests and their descriptions. Used solely for pretty printing
/// purposes in `clap-validator list tests`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct TestList {
    pub plugin_library_tests: BTreeMap<String, String>,
    pub plugin_tests: BTreeMap<String, String>,
}

/// An abstraction for a test case. This mostly exists because we need two separate kinds of tests
/// (per library and per plugin), and it's good to keep the interface uniform.
pub trait TestCase<'a>: Display + FromStr + IntoEnumIterator + Sized + 'static {
    /// The type of the arguments the test cases are parameterized over. This can be an instance of
    /// the plugin library and a plugin ID, or just the file path to the plugin library.
    type TestArgs;

    /// Get the textual description for a test case. This description won't contain any line breaks,
    /// but it may consist of multiple sentences.
    fn description(&self) -> String;

    /// Set the arguments for `clap-validator run-single-test` to run this test with the specified
    /// arguments. This way the [`run_out_of_process()`][Self::run_out_of_process()] method can be
    /// defined in a way that works for all `TestCase`s.
    fn set_out_of_process_args(&self, command: &mut Command, args: Self::TestArgs);

    /// Run a test case for a specified arguments in the current, returning the result. If the test
    /// cuases the plugin to segfault, then this will obviously not return. See
    /// [`run_out_of_process()`][Self::run_out_of_process()] for a generic way to run test cases in
    /// a separate process.
    ///
    /// In the event that this is called for a plugin ID that does not exist within the plugin
    /// library, then the test will also be marked as failed.
    fn run_in_process(&self, args: Self::TestArgs) -> TestResult;

    /// Run a test case for a plugin in another process, returning the result. If the test cuases the
    /// plugin to segfault, then the result will have a status of `TestStatus::Crashed`. If
    /// `hide_output` is set, then the tested plugin's output will not be printed to STDIO.
    ///
    /// The verbosity option is threaded through here so out of process tests use the same logger
    /// verbosity as in-process tests.
    ///
    /// In the event that this is called for a plugin ID that does not exist within the plugin
    /// library, then the test will also be marked as failed.
    ///
    /// This will only return an error if the actual `clap-validator` process call failed.
    fn run_out_of_process(
        &self,
        args: Self::TestArgs,
        verbosity: Verbosity,
        hide_output: bool,
    ) -> Result<TestResult> {
        // The idea here is that we'll invoke the same clap-validator binary with a special hidden command
        // that runs a single test. This is the reason why test cases must be convertible to and
        // from strings. If everything goes correctly, then the child process will write the results
        // as JSON to the specified file path. This is intentionaly not done through STDIO since the
        // hosted plugin may also write things there, and doing STDIO redirection within the child
        // process is more complicated than just writing the result to a temporary file.

        // This temporary file will automatically be removed when this function exits
        let output_file_path = tempfile::Builder::new()
            .suffix(".json")
            .tempfile()
            .context("Could not create a temporary file path")?
            .into_temp_path();
        let clap_validator_binary =
            std::env::current_exe().context("Could not find the path to the current executable")?;
        let mut command = Command::new(clap_validator_binary);

        command
            .arg("--verbosity")
            .arg(verbosity.to_possible_value().unwrap().get_name())
            .arg("run-single-test")
            .args([OsStr::new("--output-file"), output_file_path.as_os_str()]);
        self.set_out_of_process_args(&mut command, args);
        if hide_output {
            command.stdout(Stdio::null());
            command.stderr(Stdio::null());
        }

        let exit_status = command
            .spawn()
            .context("Could not call clap-validator for out-of-process validation")?
            // The docs make it seem like this can only fail if the process isn't running, but if
            // spawn succeeds then this can never fail:
            .wait()
            .context("Error while waiting on clap-validator to finish running the test")?;
        if !exit_status.success() {
            return Ok(TestResult {
                name: self.to_string(),
                description: self.description(),
                status: TestStatus::Crashed {
                    details: exit_status.to_string(),
                },
            });
        }

        // At this point, the child process _should_ have written its output to `output_file_path`,
        // and we can just parse it from there
        let result =
            serde_json::from_str(&fs::read_to_string(&output_file_path).with_context(|| {
                format!(
                    "Could not read the child process output from '{}'",
                    output_file_path.display()
                )
            })?)
            .context("Could not parse the child process output to JSON")?;

        Ok(result)
    }

    /// Get a writable temporary file handle for this test case. The file will be located at
    /// `$TMP_DIR/clap-validator/$plugin_id/$test_name/$file_name`. The temporary files directory is
    /// cleared on a new validator run, but the files will persist until then.
    fn temporary_file(&self, plugin_id: &str, name: &str) -> Result<(PathBuf, fs::File)> {
        let path = util::validator_temp_dir()
            .join(plugin_id)
            .join(self.to_string())
            .join(name);
        if path.exists() {
            panic!(
                "Tried to create a temporary file at '{}', but this file already exists. This is \
                 a bug in clap-validator.",
                path.display()
            )
        }

        fs::create_dir_all(path.parent().unwrap())
            .context("Could not create the directory for the test's temporary files")?;
        let file =
            fs::File::create(&path).context("Could not create a temporary file for the test")?;

        Ok((path, file))
    }

    /// Create a [`TestResult`] for this test case. The test status is wrapped in an anyhow
    /// [`Result`] to make writing test cases more ergonomic using the question mark operator. `Err`
    /// values are converted to [`TestStatus::Failed`] statuses containing the full error backtrace.
    fn create_result(&self, status: Result<TestStatus>) -> TestResult {
        TestResult {
            name: self.to_string(),
            description: self.description(),
            status: status.unwrap_or_else(|err| TestStatus::Failed {
                details: Some(format!("{err:#}")),
            }),
        }
    }
}

impl TestStatus {
    /// Returns `true` if tests with this status should be shown when running the validator with the
    /// `--only-failed` option.
    pub fn failed_or_warning(&self) -> bool {
        match self {
            TestStatus::Success { .. } | TestStatus::Skipped { .. } => false,
            TestStatus::Warning { .. } | TestStatus::Crashed { .. } | TestStatus::Failed { .. } => {
                true
            }
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

impl Default for TestList {
    fn default() -> Self {
        Self {
            plugin_library_tests: PluginLibraryTestCase::iter()
                .map(|c| (c.to_string(), c.description()))
                .collect(),
            plugin_tests: PluginTestCase::iter()
                .map(|c| (c.to_string(), c.description()))
                .collect(),
        }
    }
}
