//! Commands for validating plugins.

use crate::cli::{Config, Report, ReportItem, pluralize};
use crate::tests::{TestGroup, TestResult, TestStatus};
use crate::validator::{ValidationResult, ValidationTally};
use crate::{Verbosity, validator};
use anyhow::{Context, Result};
use clap::Args;
use std::path::PathBuf;
use std::process::ExitCode;
use yansi::Paint;

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
    #[arg(short = 'p', long)]
    pub plugin_id: Option<String>,
    /// Print the test output as JSON instead of human readable text.
    #[arg(long)]
    pub json: bool,
    /// Only run the tests that match this case-insensitive regular expression.
    /// Multiple include patterns can be passed, in which case a test only needs to match one of them to be included.
    #[arg(short = 't', long)]
    pub include: Vec<String>,
    /// Don't run the tests that match this case-insensitive regular expression.
    /// Exclude takes precedence over include, so if a test matches both, it will be excluded.
    #[arg(short = 'x', long)]
    pub exclude: Vec<String>,
    /// When running the validation out-of-process, hide the plugin's output.
    ///
    /// This can be useful for validating noisy plugins.
    #[arg(long, conflicts_with = "in_process")]
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
    /// Set the amount of parallelism when running the tests. Only for out-of-process tests.
    #[arg(long, short = 'j', conflicts_with = "in_process")]
    pub jobs: Option<usize>,
    /// When running the validation in-process, emit a JSON trace file that can be viewed with
    /// Chrome's tracing viewer or <https://ui.perfetto.dev>.
    ///
    /// This has a non-negligible performance impact.
    #[arg(long, requires = "in_process")]
    pub trace: bool,
}

/// The main validator command. This will validate one or more plugins and print the results.
pub fn validate(verbosity: Verbosity, settings: ValidatorSettings) -> Result<ExitCode> {
    let config = Config::from_current()?;

    let mut result = validator::validate(verbosity, &settings, &config).context("Could not run the validator")?;
    let tally = result.tally();

    if settings.only_failed {
        result = result.filter(|test| test.status.failed_or_warning());
    }

    if settings.json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        pretty_print(&result, &tally);
    }

    // If any of the tests failed, this process should exit with a failure code
    if tally.num_failed == 0 {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::FAILURE)
    }
}

fn pretty_print(result: &ValidationResult, tally: &ValidationTally) {
    fn report_test(result: &TestResult) -> Report {
        let status_text = match result.status {
            TestStatus::Success { .. } => "PASSED".green(),
            TestStatus::Skipped { .. } => "SKIPPED".dim(),
            TestStatus::Warning { .. } => "WARNING".yellow(),
            TestStatus::Failed { .. } => "FAILED".red(),
            TestStatus::Crashed { .. } => "CRASHED".red().bold(),
        };

        let mut items = vec![ReportItem::Text(result.test.description())];

        if let Some(details) = result.status.details() {
            items.push(ReportItem::Child(Report {
                header: "".to_string(),
                footer: vec![],
                items: vec![ReportItem::Text(details.to_string())],
            }));
        }

        Report {
            items,
            header: result.test.name(),
            footer: vec![
                status_text.to_string(),
                format!("{}ms", result.duration.as_millis()).dim().to_string(),
            ],
        }
    }

    for (group, tests) in result.group() {
        match group {
            TestGroup::PluginLibrary(library_path) => {
                let mut items = vec![ReportItem::Text(library_path.to_string_lossy().to_string())];

                for test in &tests {
                    items.push(ReportItem::Child(report_test(test)));
                }

                println!(
                    "\n{}",
                    Report {
                        header: "Plugin Library".to_string(),
                        footer: vec![pluralize(tests.len(), "test")],
                        items,
                    }
                );
            }

            TestGroup::PluginInstance(_, plugin_id) => {
                let mut items = vec![ReportItem::Text(plugin_id.clone())];

                for test in &tests {
                    items.push(ReportItem::Child(report_test(test)));
                }

                println!(
                    "\n{}",
                    Report {
                        header: "Plugin".to_string(),
                        footer: vec![pluralize(tests.len(), "test")],
                        items,
                    }
                );
            }
        }
    }

    println!(
        "{} run, {} passed, {} failed, {} warnings, {} skipped",
        pluralize(tally.total(), "test"),
        tally.num_passed.green().bold(),
        tally.num_failed.red().bold(),
        tally.num_warnings.yellow().bold(),
        tally.num_skipped.bold(),
    );
}
