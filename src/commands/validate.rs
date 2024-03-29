//! Commands for validating plugins.

use std::process::ExitCode;

use anyhow::{Context, Result};
use colored::Colorize;

use super::{println_wrapped, TextWrapper};
use crate::tests::TestStatus;
use crate::validator::{self, SingleTestSettings, ValidatorSettings};
use crate::Verbosity;

/// The main validator command. This will validate one or more plugins and print the results.
pub fn validate(verbosity: Verbosity, settings: &ValidatorSettings) -> Result<ExitCode> {
    let mut result =
        validator::validate(verbosity, settings).context("Could not run the validator")?;
    let tally = result.tally();

    // Filtering out tests should be done after we did the tally for consistency's sake
    if settings.only_failed {
        // The `.drain_filter()` methods have not been stabilized yet, so to make things
        // easy for us we'll just inefficiently rebuild the data structures
        result.plugin_library_tests = result
            .plugin_library_tests
            .into_iter()
            .filter_map(|(library_path, tests)| {
                let tests: Vec<_> = tests
                    .into_iter()
                    .filter(|test| test.status.failed_or_warning())
                    .collect();
                if tests.is_empty() {
                    None
                } else {
                    Some((library_path, tests))
                }
            })
            .collect();

        result.plugin_tests = result
            .plugin_tests
            .into_iter()
            .filter_map(|(plugin_id, tests)| {
                let tests: Vec<_> = tests
                    .into_iter()
                    .filter(|test| test.status.failed_or_warning())
                    .collect();
                if tests.is_empty() {
                    None
                } else {
                    Some((plugin_id, tests))
                }
            })
            .collect();
    }

    if settings.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&result).expect("Could not format JSON")
        );
    } else {
        let mut wrapper = TextWrapper::default();
        // This doesn't need to be a macro but the alternatives are to either wrap `wrapper` in a
        // refcell or to inline this, so this is probably still better
        macro_rules! print_test {
            ($test:expr) => {
                println_wrapped!(wrapper, "   - {}: {}", $test.name, $test.description);

                let status_text = match $test.status {
                    TestStatus::Success { .. } => "PASSED".green(),
                    TestStatus::Crashed { .. } => "CRASHED".red().bold(),
                    TestStatus::Failed { .. } => "FAILED".red(),
                    TestStatus::Skipped { .. } => "SKIPPED".yellow(),
                    TestStatus::Warning { .. } => "WARNING".yellow(),
                };
                let test_result = match $test.status.details() {
                    Some(reason) => format!("     {status_text}: {reason}"),
                    None => format!("     {status_text}"),
                };
                wrapper.print_auto(test_result);
            };
        }

        if !result.plugin_library_tests.is_empty() {
            println!("Plugin library tests:");
            for (library_path, tests) in result.plugin_library_tests {
                println!();
                println_wrapped!(wrapper, " - {}", library_path.display());

                for test in tests {
                    println!();
                    print_test!(test);
                }
            }

            println!();
        }

        if !result.plugin_tests.is_empty() {
            println!("Plugin tests:");
            for (plugin_id, tests) in result.plugin_tests {
                println!();
                println_wrapped!(wrapper, " - {plugin_id}");

                for test in tests {
                    println!();
                    print_test!(test);
                }
            }

            println!();
        }

        let num_tests = tally.total();
        println_wrapped!(
            wrapper,
            "{} {} run, {} passed, {} failed, {} skipped, {} warnings",
            num_tests,
            if num_tests == 1 { "test" } else { "tests" },
            tally.num_passed,
            tally.num_failed,
            tally.num_skipped,
            tally.num_warnings
        );
    }

    // If any of the tests failed, this process should exit with a failure code
    if tally.num_failed == 0 {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::FAILURE)
    }
}

/// Run a single test and write the output to a file. This command is a hidden implementation detail
/// used by the validator to run tests in a different process.
pub fn run_single(settings: &SingleTestSettings) -> Result<ExitCode> {
    // The result will be serialized as JSON and written to a file so the main validator process can
    // read it
    validator::run_single_test(settings)
        .map(|()| ExitCode::SUCCESS)
        .context("Could not run test the case")
}
