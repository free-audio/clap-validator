//! A tracing layer that logs events to standard output in a compact human readable format.

use crate::cli::tracing::{event, record};
use std::cell::RefCell;
use std::fmt::Write;
use std::sync::OnceLock;
use std::time::SystemTime;
use yansi::Paint;

/// The time of origin for the log timestamps.
/// Timestamps are logged as '{}ms' where the number of milliseconds is the duration since this timestamp.
pub fn timebase() -> SystemTime {
    static TIMEBASE: OnceLock<SystemTime> = OnceLock::new();
    *TIMEBASE.get_or_init(|| {
        std::env::var("CLAP_VALIDATOR_TIMEBASE")
            .ok()
            .and_then(|s| s.parse::<f64>().ok())
            .map(|secs| SystemTime::UNIX_EPOCH + std::time::Duration::from_secs_f64(secs))
            .unwrap_or_else(SystemTime::now)
    })
}

pub struct CustomLogger;

impl log::Log for CustomLogger {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }

    fn log(&self, log: &log::Record) {
        thread_local! {
            static BUFFER: RefCell<String> = const { RefCell::new(String::new()) }
        }

        let elapsed = SystemTime::now()
            .duration_since(timebase())
            .unwrap_or_default()
            .as_secs_f64()
            * 1000.0;

        let prefix = match log.level() {
            log::Level::Error => "ERROR".red().bold(),
            log::Level::Warn => " WARN".yellow(),
            log::Level::Info => " INFO".green(),
            log::Level::Debug => "DEBUG".blue(),
            log::Level::Trace => "TRACE".white(),
        };

        event(
            log.args(),
            record! {
                level: log.level().to_string(),
                target: log.target()
            },
        );

        BUFFER.with_borrow_mut(|buffer| {
            buffer.clear();
            write!(buffer, "{:>5.0}{}", elapsed.dim(), "ms".dim()).ok();
            write!(buffer, " {} ", prefix).ok();
            write!(buffer, "{}", log.args()).ok();
            write!(buffer, " {}", log.target().dim().italic()).ok();
            writeln!(buffer).ok();
            eprint!("{}", buffer);
        });
    }

    fn flush(&self) {}
}
