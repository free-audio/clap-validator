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
use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::plugin::library::ClapPluginLibrary;

/// The string representation for [`TestCase::PluginScanTime`].
pub const TEST_PLUGIN_SCAN_TIME: &str = "plugin-scan-time";

pub const PLUGIN_SCAN_TIME_LIMIT: Duration = Duration::from_millis(100);

/// A test case for testing the behavior of a plugin. This `Test` object contains the result of a
/// test, which is serialized to and from JSON so the test can be run in another process.
#[derive(Debug, Deserialize, Serialize)]
pub struct TestResult {
    /// The name of this test.
    pub name: String,
    /// A description of what this test case has tested.
    pub description: String,
    /// The outcome of the test.
    pub result: TestStatus,
}

/// The result of running a test. Skipped and failed test may optionally include an explanation for
/// why this happened.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
#[serde(tag = "status")]
pub enum TestStatus {
    /// The test passed successfully.
    Success,
    /// The plugin segfaulted, SIGABRT'd, or otherwise crashed while running the test. This is only
    /// caught for out-of-process validation, for obvious reasons.
    Crashed { reason: String },
    /// The test failed.
    Failed { reason: Option<String> },
    /// Preconditions for running the test were not met, so the test has been skipped.
    Skipped { reason: Option<String> },
}

/// Tests for entire CLAP libraries. These are mostly to ensure good plugin scanning practices. See
/// the module's heading for more information.
pub enum PluginLibraryTestCase {
    /// Asserts whether the plugin takes longer than `PLUGIN_SCAN_TIME_LIMIT` to scan.
    PluginScanTime,
}

/// The tests for individual CLAP plugins. See the module's heading for more information.
pub enum PluginTestCase {
    // TODO: Test some things
}

/// An abstraction for a test case. This mostly exists because we need two separate kinds of tests
/// (per library and per plugin), and it's good to keep the interface uniform.
pub trait TestCase<'a>: Sized + 'static {
    /// The type of the arguments the test cases are parameterized over. This can be an instance of
    /// the plugin library and a plugin ID, or just the file path to the plugin library.
    type TestArgs;

    /// All available test cases.
    const ALL: &'static [Self];

    /// Try to parse a test case's string representation as produced by
    /// [`as_str()`][Self::as_str()]. Returns `None` if the test case name was not recognized.
    fn from_str(string: &str) -> Option<Self>;

    /// Get the string representation of this test case.
    fn as_str(&self) -> &'static str;

    /// Get the textual description for a test case. This description won't contain any line breaks,
    /// but it may consist of multiple sentences.
    fn description(&self) -> String;

    /// Set the arguments for `clapval run-single-test` to run this test with the specified
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
    /// In the event that this is called for a plugin ID that does not exist within the plugin
    /// library, then the test will also be marked as failed.
    ///
    /// This will only return an error if the actual `clapval` process call failed.
    fn run_out_of_process(&self, args: Self::TestArgs, hide_output: bool) -> Result<TestResult> {
        // The idea here is that we'll invoke the same clapval binary with a special hidden command
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
        let clapval_binary =
            std::env::current_exe().context("Could not find the path to the current executable")?;
        let mut command = Command::new(clapval_binary);

        command
            .arg("run-single-test")
            .args([OsStr::new("--output-file"), output_file_path.as_os_str()]);
        self.set_out_of_process_args(&mut command, args);
        if hide_output {
            command.stdout(Stdio::null());
            command.stderr(Stdio::null());
        }

        let exit_status = command
            .spawn()
            .context("Could not call clapval for out-of-process validation")?
            // The docs make it seem like this can only fail if the process isn't running, but if
            // spawn succeeds then this can never fail:
            .wait()
            .context("Error while waiting on clapval to finish running the test")?;
        if !exit_status.success() {
            return Ok(TestResult {
                name: self.as_str().to_string(),
                description: self.description(),
                result: TestStatus::Crashed {
                    reason: exit_status.to_string(),
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
}

impl<'a> TestCase<'a> for PluginLibraryTestCase {
    /// The path to a CLAP plugin library.
    type TestArgs = &'a Path;

    const ALL: &'static [Self] = &[PluginLibraryTestCase::PluginScanTime];

    fn from_str(string: &str) -> Option<Self> {
        match string {
            TEST_PLUGIN_SCAN_TIME => Some(PluginLibraryTestCase::PluginScanTime),
            _ => None,
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            PluginLibraryTestCase::PluginScanTime => TEST_PLUGIN_SCAN_TIME,
        }
    }

    fn description(&self) -> String {
        match self {
            PluginLibraryTestCase::PluginScanTime => format!(
                "Tests whether the plugin can be scanned in under {} milliseconds.",
                PLUGIN_SCAN_TIME_LIMIT.as_millis()
            ),
        }
    }

    fn set_out_of_process_args(&self, command: &mut Command, library_path: Self::TestArgs) {
        let test_name = self.as_str();

        command
            .arg(
                super::SingleTestType::PluginLibrary
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
        let result = match &self {
            PluginLibraryTestCase::PluginScanTime => {
                let test_start = Instant::now();

                eprintln!("TODO: Actually implement the plugin scanning time");

                let test_end = Instant::now();
                let init_duration = test_end - test_start;
                if init_duration <= PLUGIN_SCAN_TIME_LIMIT {
                    TestStatus::Success
                } else {
                    TestStatus::Failed {
                        reason: Some(format!(
                            "The plugin took {} milliseconds to scan",
                            init_duration.as_millis()
                        )),
                    }
                }
            }
        };

        TestResult {
            name: self.as_str().to_string(),
            description: self.description(),
            result,
        }
    }
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
                super::SingleTestType::Plugin
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
