use crate::cli::{Report, ReportItem};
use crate::commands::Verbosity;
use crate::fuzz::{FuzzResult, FuzzStatus};
use anyhow::Result;
use clap::Args;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;
use std::vec;
use yansi::Paint;

fn parse_duration(mut str: &str) -> Result<Duration, &'static str> {
    if str.is_empty() {
        return Err("no duration provided");
    }

    let mut duration = Duration::from_secs(0);
    while !str.is_empty() {
        let (num, rest) = str
            .trim_ascii_start()
            .split_at(str.find(|c: char| !c.is_ascii_digit()).unwrap_or(str.len()));
        let (unit, rest) = rest
            .trim_ascii_start()
            .split_at(rest.find(|c: char| c.is_ascii_digit()).unwrap_or(rest.len()));

        let num: u64 = num.parse::<u64>().map_err(|_| "invalid duration format")?;
        let unit = match unit {
            "ms" | "millis" => Duration::from_millis(num),
            "s" | "sec" | "seconds" => Duration::from_secs(num),
            "m" | "min" | "minutes" => Duration::from_secs(num * 60),
            "h" | "hr" | "hrs" | "hour" | "hours" => Duration::from_secs(num * 60 * 60),
            _ => return Err("invalid duration format"),
        };

        duration += unit;
        str = rest;
    }

    Ok(duration)
}

/// Options for the fuzzer.
#[derive(Debug, Args)]
pub struct FuzzSettings {
    /// Paths to one or more plugins that should be fuzzed.
    #[arg(required = true)]
    pub paths: Vec<PathBuf>,

    /// Only fuzz plugins with this ID.
    ///
    /// If the plugin library contains multiple plugins, then you can pass a single plugin's ID
    /// to this option to only fuzz that plugin. Otherwise all plugins in the library are
    /// fuzzed.
    #[arg(short = 'p', long)]
    pub plugin_id: Option<String>,

    /// Print the test output as JSON instead of human readable text.
    #[arg(long)]
    pub json: bool,

    /// Run the fuzzer for this long before stopping.
    /// By default it will run until stopped manually via Ctrl+C.
    #[arg(long, short = 'd', value_parser = parse_duration)]
    pub duration: Option<Duration>,

    /// When running the fuzzer out-of-process, this many fuzzing chunks will be run in parallel.
    ///
    /// By default this is set to the number of logical CPU cores.
    #[arg(long, short = 'j')]
    pub jobs: Option<usize>,

    /// When running the validation in-process, emit a JSON trace file that can be viewed with
    /// Chrome's tracing viewer or <https://ui.perfetto.dev>.
    ///
    /// This has a non-negligible performance impact.
    #[arg(long, requires = "reproduce")]
    pub trace: bool,

    /// How many errors to collect before stopping the fuzzer.
    #[arg(long, short = 'l', default_value = "1")]
    pub limit: usize,

    /// Run the fuzzer with this random seed in-process.
    ///
    /// This will run a single deterministic fuzzing chunk that will execute the same sequence of calls every time.
    /// Useful for reproducing an error/crash produced by the out-of-process fuzzer.
    #[arg(
        long,
        short = 'r',
        conflicts_with = "jobs",
        conflicts_with = "duration",
        conflicts_with = "limit"
    )]
    pub reproduce: Option<u64>,
}

/// The main fuzzer command. This will fuzz one or more plugins and print the results.
pub fn fuzz(verbosity: Verbosity, settings: FuzzSettings) -> Result<ExitCode> {
    let result = crate::fuzz::fuzz(verbosity, &settings)?;

    if settings.json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else if result.is_empty() {
        eprintln!("{}: fuzzing finished successfully", "OK".green().bold());
    } else {
        pretty_print(&result);
    }

    if result.iter().all(|result| result.status == FuzzStatus::Success) {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::FAILURE)
    }
}

pub fn pretty_print(result: &[FuzzResult]) {
    for result in result {
        let status_text = match result.status {
            FuzzStatus::Success => "OK".green(),
            FuzzStatus::Failed { .. } => "FAILED".red(),
            FuzzStatus::Crashed { .. } => "CRASHED".red().bold(),
        };

        let details = match &result.status {
            FuzzStatus::Success => None,
            FuzzStatus::Failed { details } => Some(details),
            FuzzStatus::Crashed { details } => Some(details),
        };

        let mut report = Report {
            header: result.plugin_id.to_string(),
            footer: vec![status_text.to_string(), result.seed.dim().to_string()],
            items: vec![],
        };

        if let Some(details) = details {
            report.items.push(ReportItem::Text(details.to_string()));
        }

        eprintln!("\n{}", report);
    }
}
