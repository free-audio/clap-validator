#![allow(clippy::needless_range_loop)]

use commands::{Arguments, Command, Verbosity};
use std::process::ExitCode;
use yansi::Paint;

mod cli;
mod commands;
mod fuzz;
mod plugin;
mod tests;
mod validator;

fn main() -> ExitCode {
    let args = <Arguments as clap::Parser>::parse();

    if !matches!(args.command, Command::Sandbox(_)) {
        // Before doing anything, we need to make sure any temporary artifact files from the previous
        // run are cleaned up. These are used for things like state dumps when one of the state tests
        // fail. This is allowed to fail since the directory may not exist and even if it does and we
        // cannot remove it, then that may not be a problem.
        let _ = std::fs::remove_dir_all(cli::validator_temp_dir());
        let _ = std::fs::create_dir_all(cli::validator_temp_dir());
    }

    // begin instrumentation if enabled
    let trace_path = cli::validator_temp_dir().join("trace.json");
    let trace_enabled = match &args.command {
        Command::Validate(settings) => settings.trace,
        Command::Fuzz(settings) => settings.trace,
        _ => false,
    };

    if trace_enabled {
        cli::tracing::install(&trace_path);
    }

    // setup logging
    log::set_logger(&cli::CustomLogger).unwrap();
    log::set_max_level(match args.verbosity {
        Verbosity::Quiet => log::LevelFilter::Off,
        Verbosity::Error => log::LevelFilter::Error,
        Verbosity::Warn => log::LevelFilter::Warn,
        Verbosity::Info => log::LevelFilter::Info,
        Verbosity::Debug => log::LevelFilter::Debug,
        Verbosity::Trace => log::LevelFilter::Trace,
    });

    // install the panic hook to log panics instead of printing them to stderr directly
    cli::install_panic_hook();

    // mark the main thread as such for plugin instance creation checks
    unsafe {
        plugin::library::mark_current_thread_as_os_main_thread();
    }

    let result = match args.command {
        Command::Validate(settings) => commands::validate::validate(args.verbosity, settings),
        Command::Fuzz(settings) => commands::fuzz::fuzz(args.verbosity, settings),
        Command::List(command) => commands::list::list(args.verbosity, command),
        Command::Sandbox(payload) => {
            payload.dispatch();
            Ok(ExitCode::SUCCESS)
        }
    };

    let status = match &result {
        Ok(code) => *code,
        Err(err) => {
            eprintln!("{} {err:#}", "Error:".red().bold());
            ExitCode::FAILURE
        }
    };

    if trace_enabled {
        match cli::tracing::check_error() {
            Err(e) => eprintln!("{}: {}", "Failed to write trace".red().italic(), e),
            Ok(()) => eprintln!(
                "{}",
                format!(
                    "Trace written to '{}'. Go to https://ui.perfetto.dev/ to view it.",
                    trace_path.display()
                )
                .dim()
                .italic()
            ),
        }
    }

    status
}
