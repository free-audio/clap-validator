use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use std::process::ExitCode;
use validator::{SingleTestSettings, ValidatorSettings};

mod commands;
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
    command: Command,
}

/// The verbosity level. Set to `Debug` by default. `Trace` can be used to get more information on
/// what the validator is actually doing.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Verbosity {
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
enum Command {
    /// Validate one or more plugins.
    Validate(ValidatorSettings),
    /// Run a single test.
    ///
    /// This is used for the out-of-process testing. Since it's merely an implementation detail, the
    /// option is not shown in the CLI.
    #[command(hide = true)]
    RunSingleTest(SingleTestSettings),

    #[command(subcommand)]
    List(ListCommand),
}

/// Commands for listing tests and data realted to the installed plugins.
#[derive(Subcommand)]
pub enum ListCommand {
    /// Lists basic information about all installed CLAP plugins.
    Plugins {
        /// Print JSON instead of a human readable format.
        #[arg(short, long)]
        json: bool,
    },
    /// Lists the available presets for one, more, or all installed CLAP plugins.
    Presets {
        /// Print JSON instead of a human readable format.
        #[arg(short, long)]
        json: bool,
        /// Paths to one or more plugins that should be indexed for presets, optional.
        ///
        /// All installed plugins are crawled if this value is missing.
        paths: Option<Vec<PathBuf>>,
    },
    /// Lists all available test cases.
    Tests {
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

    let result = match cli.command {
        Command::Validate(settings) => commands::validate::validate(cli.verbosity, &settings),
        Command::RunSingleTest(settings) => commands::validate::run_single(&settings),
        Command::List(ListCommand::Plugins { json }) => commands::list::plugins(json),
        Command::List(ListCommand::Presets { json, paths }) => {
            commands::list::presets(json, paths.as_deref())
        }
        Command::List(ListCommand::Tests { json }) => commands::list::tests(json),
    };

    match result {
        Ok(exit_code) => exit_code,
        Err(err) => {
            log::error!("{err:?}");
            ExitCode::FAILURE
        }
    }
}
