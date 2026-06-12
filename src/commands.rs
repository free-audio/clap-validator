//! All the different commands for the cli. Split up into modules and functions to make it a bit
//! easier to navigate.

pub mod fuzz;
pub mod list;
pub mod validate;

use clap::*;

#[derive(Parser)]
#[command(author, version, about, long_about = None, propagate_version = true)]
pub struct Arguments {
    /// clap-validator's own logging verbosity.
    ///
    /// This can be used to silence all non-essential output, or to enable more in depth tracing.
    #[arg(short, long, default_value = "info")]
    pub verbosity: Verbosity,

    #[command(subcommand)]
    pub command: Command,
}

/// The validator's subcommands.
#[derive(Subcommand)]
pub enum Command {
    /// Validate one or more plugins.
    Validate(validate::ValidatorSettings),

    /// Fuzz a plugin.
    Fuzz(fuzz::FuzzSettings),

    /// List available tests, scan plugins, presets, etc.
    #[command(subcommand)]
    List(list::ListCommand),

    #[command(hide = true)]
    Sandbox(crate::cli::sandbox::SandboxPayload),
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
