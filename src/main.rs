use clap::{Parser, Subcommand, ValueEnum};
use colored::Colorize;
use std::process::ExitCode;
use validator::{SingleTestSettings, ValidatorSettings};

use crate::tests::TestResult;

mod host;
mod index;
mod plugin;
mod tests;
mod util;
mod validator;

#[derive(Parser)]
#[command(author, version, about, long_about = None, propagate_version = true)]
struct Cli {
    /// clap-validator's own logging verbosity.
    ///
    /// This can be used to silence all non-essential output, or to enable more in depth tracing.
    #[arg(short, long, default_value = "debug")]
    verbosity: Verbosity,

    #[command(subcommand)]
    command: Commands,
}

/// The verbosity level. Set to `Debug` by default. `Trace` can be used to get more information on
/// what the validator is actually doing.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum Verbosity {
    /// Suppress all logging output from the validator itself.
    Quiet,
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

/// The validator's subcommands.
#[derive(Subcommand)]
enum Commands {
    /// Validate one or more plugins.
    Validate(ValidatorSettings),
    /// Run a single test.
    ///
    /// This is used for the out-of-process testing. Since it's merely an implementation detail, the
    /// option is not shown in the CLI.
    #[command(hide = true)]
    RunSingleTest(SingleTestSettings),
    /// Subcommands for listing data about the tests or the installed plugins.
    #[command(subcommand)]
    List(ListCommands),
}

/// Subcommands for listing data about the tests or the installed plugins.
#[derive(Subcommand)]
enum ListCommands {
    /// Lists basic information about all installed CLAP plugins.
    Plugins {
        /// Print JSON instead of a human readable format.
        #[arg(short, long)]
        json: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    // For now logging everything to the terminal is fine. In the future it may be useful to have
    // CLI options for things like the verbosity level.
    simplelog::TermLogger::init(
        match cli.verbosity {
            Verbosity::Quiet => simplelog::LevelFilter::Off,
            Verbosity::Error => simplelog::LevelFilter::Error,
            Verbosity::Warn => simplelog::LevelFilter::Warn,
            Verbosity::Info => simplelog::LevelFilter::Info,
            Verbosity::Debug => simplelog::LevelFilter::Debug,
            Verbosity::Trace => simplelog::LevelFilter::Trace,
        },
        simplelog::ConfigBuilder::new()
            .set_thread_mode(simplelog::ThreadLogMode::Both)
            .set_location_level(simplelog::LevelFilter::Debug)
            .build(),
        simplelog::TerminalMode::Stderr,
        simplelog::ColorChoice::Auto,
    )
    .expect("Could not initialize logger");
    log_panics::init();

    match &cli.command {
        Commands::Validate(settings) => match validator::validate(settings) {
            Ok(mut result) => {
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
                                .filter(|test| test.status.failed())
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
                                .filter(|test| test.status.failed())
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
                    let wrapping_options =
                        textwrap::Options::with_termwidth().subsequent_indent("         ");
                    let print_wrapped = |text: String| {
                        println!("{}", textwrap::fill(&text, wrapping_options.clone()))
                    };
                    let print_test = |test: TestResult| {
                        // TODO: We may want to wrap this for the terminal
                        print_wrapped(format!("   - {}: {}", test.name, test.description));

                        let status_text = match test.status {
                            tests::TestStatus::Success { .. } => "PASSED".green(),
                            tests::TestStatus::Crashed { .. } => "CRASHED".red().bold(),
                            tests::TestStatus::Failed { .. } => "FAILED".red(),
                            tests::TestStatus::Skipped { .. } => "SKIPPED".yellow(),
                            tests::TestStatus::Warning { .. } => "WARNING".yellow(),
                        };
                        let test_result = match test.status.details() {
                            Some(reason) => format!("     {}: {}", status_text, reason),
                            None => format!("     {}", status_text),
                        };
                        print_wrapped(test_result);
                    };

                    if !result.plugin_library_tests.is_empty() {
                        println!("Plugin library tests:");
                        for (library_path, tests) in result.plugin_library_tests {
                            println!();
                            println!(" - {}", library_path.display());

                            for test in tests {
                                println!();
                                print_test(test);
                            }
                        }

                        println!();
                    }

                    if !result.plugin_tests.is_empty() {
                        println!("Plugin tests:");
                        for (plugin_id, tests) in result.plugin_tests {
                            println!();
                            println!(" - {}", plugin_id);

                            for test in tests {
                                println!();
                                print_test(test);
                            }
                        }

                        println!();
                    }

                    let num_tests = tally.total();
                    println!(
                        "{} {} run, {} passed, {} failed, {} skipped, {} warnings",
                        num_tests,
                        if num_tests == 1 { "test" } else { "tests" },
                        tally.num_passed,
                        tally.num_failed,
                        tally.num_skipped,
                        tally.num_warnings
                    )
                }

                // If any of the tests failed, this process should exiti with a failure code
                if tally.num_failed > 0 {
                    return ExitCode::FAILURE;
                }
            }
            Err(err) => log::error!("Could not run the validator: {err:#}"),
        },
        Commands::RunSingleTest(settings) => match validator::run_single_test(settings) {
            // The result has been serialized as JSON and written to a file so the main validator
            // process can read it
            Ok(()) => (),
            Err(err) => log::error!("Could not run test the case: {err:#}"),
        },
        Commands::List(ListCommands::Plugins { json }) => {
            let plugin_index = index::index();

            if *json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&plugin_index).expect("Could not format JSON")
                );
            } else {
                for (i, (plugin_path, metadata)) in plugin_index.0.into_iter().enumerate() {
                    if i > 0 {
                        println!();
                    }

                    println!(
                        "{}: (CLAP {}.{}.{}, contains {} {})",
                        plugin_path.display(),
                        metadata.version.0,
                        metadata.version.1,
                        metadata.version.2,
                        metadata.plugins.len(),
                        if metadata.plugins.len() == 1 {
                            "plugin"
                        } else {
                            "plugins"
                        },
                    );

                    for plugin in metadata.plugins {
                        println!();
                        println!(
                            " - {} {} ({})",
                            plugin.name,
                            plugin.version.as_deref().unwrap_or("(unknown version)"),
                            plugin.id
                        );

                        // Whether it makes sense to always show optional fields or not depends on
                        // the field
                        if let Some(description) = plugin.description {
                            println!("   {}", description);
                        }
                        println!();
                        println!(
                            "   vendor: {}",
                            plugin.vendor.as_deref().unwrap_or("(unknown)"),
                        );
                        if let Some(manual_url) = plugin.manual_url {
                            println!("   manual url: {}", manual_url);
                        }
                        if let Some(support_url) = plugin.support_url {
                            println!("   support url: {}", support_url);
                        }
                        println!("   features: [{}]", plugin.features.join(", "));
                    }
                }
            }
        }
    }

    ExitCode::SUCCESS
}
