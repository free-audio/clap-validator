mod rng;
mod runner;

use crate::cli::sandbox::{SandboxConfig, SandboxOperation};
use crate::cli::{IteratorExt, panic_message};
use crate::commands::Verbosity;
use crate::commands::fuzz::FuzzSettings;
use anyhow::{Context, Result};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
#[serde(tag = "type")]
pub enum FuzzStatus {
    Success,
    Failed { details: String },
    Crashed { details: String },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct FuzzResult {
    pub status: FuzzStatus,
    pub library: PathBuf,
    pub plugin_id: String,
    pub seed: u64,
}

impl FuzzStatus {
    pub fn details(&self) -> Option<&str> {
        match self {
            FuzzStatus::Success => None,
            FuzzStatus::Failed { details } | FuzzStatus::Crashed { details } => Some(details),
        }
    }
}

pub fn fuzz(verbosity: Verbosity, settings: &FuzzSettings) -> Result<Vec<FuzzResult>> {
    let plugins = discover(&settings.paths, settings.plugin_id.as_deref())?;
    if plugins.is_empty() {
        anyhow::bail!("No plugins selected");
    }

    if let Some(seed) = settings.reproduce {
        if plugins.len() > 1 {
            let plugins = plugins
                .iter()
                .map(|(library, plugin_id)| format!("\n - {} ({})", library.display(), plugin_id))
                .collect::<Vec<_>>()
                .join("");

            anyhow::bail!("Choose one out of: {}", plugins);
        }

        let (library, plugin_id) = &plugins[0];
        let status = SandboxedFuzzChunk {
            library: library.clone(),
            plugin_id: plugin_id.clone(),
            seed,
        }
        .run();

        return Ok(vec![FuzzResult {
            status,
            library: library.clone(),
            plugin_id: plugin_id.clone(),
            seed,
        }]);
    }

    // round robin over the plugins until we run out of time
    let start = Instant::now();
    let running = AtomicBool::new(true);
    let mut results = vec![];
    let mut prng = rng::new_orchestrator_prng();

    std::iter::repeat(&plugins)
        .flatten()
        .map(|(library, plugin_id)| (library, plugin_id, prng.next_u64()))
        .take_while(|_| settings.duration.is_none_or(|duration| start.elapsed() < duration)) // run while we have time
        .take_while(|_| running.load(Ordering::Relaxed)) // stop if we found a result
        .parallel_fork_join(
            settings.jobs,
            |(library, plugin_id, seed)| {
                let status = SandboxedFuzzChunk {
                    library: library.clone(),
                    plugin_id: plugin_id.clone(),
                    seed,
                }
                .run_sandboxed(SandboxConfig {
                    verbosity,
                    hide_output: false,
                    timeout: Some(std::time::Duration::from_secs(60)),
                })
                .unwrap_or_else(|err| FuzzStatus::Crashed {
                    details: err.to_string(),
                });

                FuzzResult {
                    status,
                    library: library.clone(),
                    plugin_id: plugin_id.clone(),
                    seed,
                }
            },
            |chunk| {
                if chunk.status != FuzzStatus::Success {
                    log::error!(
                        "{} ({}, seed {})",
                        chunk.status.details().unwrap_or_default(),
                        chunk.plugin_id,
                        chunk.seed,
                    );

                    results.push(chunk);

                    if results.len() >= settings.limit {
                        running.store(false, Ordering::Relaxed);
                    }
                } else {
                    log::debug!("OK '{}' (seed {})", chunk.plugin_id, chunk.seed);
                }
            },
        );

    Ok(results)
}

/// Scan the paths for plugins and return the paths and plugin IDs of the plugins that should be fuzzed.
fn discover(paths: &[PathBuf], plugin_id: Option<&str>) -> Result<Vec<(PathBuf, String)>> {
    let mut result = Vec::new();

    for path in paths {
        let library = crate::plugin::library::PluginLibrary::load(path)?;

        let metadata = library
            .metadata()
            .with_context(|| format!("Could not get the plugin metadata for library '{}'", path.display()))?;

        for plugin in metadata.plugins {
            if plugin_id.as_ref().is_none_or(|id| id == &plugin.id) {
                result.push((path.clone(), plugin.id));
            }
        }
    }

    Ok(result)
}

#[derive(Serialize, Deserialize)]
pub struct SandboxedFuzzChunk {
    library: PathBuf,
    plugin_id: String,
    seed: u64,
}

impl SandboxOperation for SandboxedFuzzChunk {
    const ID: &'static str = "fuzz";
    type Result = FuzzStatus;

    fn run(&self) -> Self::Result {
        match catch_unwind(AssertUnwindSafe(|| {
            runner::run_fuzzer(&self.library, &self.plugin_id, self.seed)
        })) {
            Ok(Ok(result)) => result,
            Ok(Err(err)) => {
                let err = err.chain().map(|x| x.to_string()).collect::<Vec<_>>().join("\n");
                FuzzStatus::Failed { details: err }
            }
            Err(panic) => FuzzStatus::Crashed {
                details: panic_message(&*panic),
            },
        }
    }
}
