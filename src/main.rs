use clap::{Parser, Subcommand};
use console::style;
use std::process::ExitCode;
use validator::{SingleTestSettings, ValidatorSettings};

use crate::tests::TestResult;

mod hosting;
mod index;
mod plugin;
mod tests;
mod util;
mod validator;

#[derive(Parser)]
#[clap(author, version, about, long_about = None)]
#[clap(propagate_version = true)]
struct Cli {
    #[clap(subcommand)]
    command: Commands,
}

/// The validator's subcommands. To be able to also add scanning functionality later (because why
/// not?), validate is a subcommand.
#[derive(Subcommand)]
enum Commands {
    /// Validate one or more plugins.
    Validate(ValidatorSettings),
    /// Run a single test.
    ///
    /// This is used for the out-of-process testing. Since it's merely an implementation detail, the
    /// option is not shown in the CLI.
    #[clap(hide(true))]
    RunSingleTest(SingleTestSettings),
    // TODO: A hidden subcommand for running a single test for a single plugin. Used by the out of
    //       process mode
    /// Lists basic information about all installed CLAP plugins.
    List {
        /// Print JSON instead of a human readable format.
        #[clap(value_parser, short, long)]
        json: bool,
    },
}

fn main() -> ExitCode {
    // For now logging everything to the terminal is fine. In the future it may be useful to have
    // CLI options for things like the verbosity level.
    simplelog::TermLogger::init(
        simplelog::LevelFilter::Trace,
        simplelog::ConfigBuilder::new()
            .set_thread_mode(simplelog::ThreadLogMode::Both)
            .set_location_level(simplelog::LevelFilter::Debug)
            .build(),
        simplelog::TerminalMode::Stderr,
        simplelog::ColorChoice::Auto,
    )
    .expect("Could not initialize logger");
    log_panics::init();

    let cli = Cli::parse();

    match &cli.command {
        Commands::Validate(settings) => match validator::validate(settings) {
            Ok(result) => {
                let tally = result.tally();
                if settings.json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&result).expect("Could not format JSON")
                    );
                } else {
                    let print_test = |test: TestResult| {
                        // TODO: We may want to wrap this for the terminal
                        println!("   - {}: {}", test.name, test.description);

                        let status_text = match test.status {
                            tests::TestStatus::Success { .. } => style("PASSED").green(),
                            tests::TestStatus::Crashed { .. } => style("CRASHED").red().bold(),
                            tests::TestStatus::Failed { .. } => style("FAILED").red(),
                            tests::TestStatus::Skipped { .. } => style("SKIPPED"),
                        };
                        match test.status.reason() {
                            Some(reason) => println!("     {}: {}", status_text, reason),
                            None => println!("     {}", status_text),
                        }
                    };

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
                    println!("Plugin tests:");
                    for (plugin_id, tests) in result.plugin_tests {
                        println!();
                        println!(" - {}", plugin_id);

                        for test in tests {
                            println!();
                            print_test(test);
                        }
                    }

                    let num_tests = tally.total();
                    println!();
                    println!(
                        "{} {} run, {} passed, {} failed, {} skipped",
                        num_tests,
                        if num_tests == 1 { "test" } else { "tests" },
                        tally.num_passed,
                        tally.num_failed,
                        tally.num_skipped
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
        Commands::List { json } => {
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
