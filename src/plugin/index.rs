//! Utilities and data structures for indexing plugins and presets.

use crate::cli::sandbox::SandboxOperation;
use crate::plugin::library::PluginMetadata;
use crate::plugin::preset_discovery::{LocationValue, PresetFile, Soundpack};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;
use walkdir::{DirEntry, WalkDir};

/// The separator for path environment variables.
#[cfg(unix)]
const PATH_SEPARATOR: char = ':';
/// The separator for path environment variables.
#[cfg(windows)]
const PATH_SEPARATOR: char = ';';

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ScannedLibrary {
    pub version: (u32, u32, u32),
    pub plugins: Vec<PluginMetadata>,
    pub preset_providers: Vec<ScannedPresets>,
}

/// Preset information declared by a preset provider.
#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub struct ScannedPresets {
    /// The preset provider's ID.
    pub provider_id: String,
    /// The preset provider's name.
    pub provider_name: String,
    /// The preset provider's vendor.
    pub provider_vendor: Option<String>,
    /// The preset provider's version.
    pub provider_version: (u32, u32, u32),
    // All sound packs declared by the plugin.
    pub soundpacks: Vec<Soundpack>,
    // All presets declared by the plugin, indexed by their location. Represented by a tuple list
    // because JSON object keys must be strings, and with the change from URIs to a location
    // kind+value that's not longer the case.
    pub presets: Vec<(LocationValue, PresetFile)>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[serde(tag = "status")]
pub enum ScanStatus {
    Success {
        #[serde(flatten)]
        library: ScannedLibrary,
        duration: Duration,
    },
    Error {
        details: String,
    },
    Crashed {
        details: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SandboxedScanLibrary {
    pub library_path: PathBuf,
    pub scan_presets: bool,
}

impl SandboxOperation for SandboxedScanLibrary {
    const ID: &'static str = "scan-library";

    type Result = ScanStatus;

    fn run(&self) -> Self::Result {
        let start = std::time::Instant::now();
        match scan_library(&self.library_path, self.scan_presets) {
            Ok(library) => ScanStatus::Success {
                library,
                duration: start.elapsed(),
            },
            Err(err) => ScanStatus::Error {
                details: format!("{:#}", err),
            },
        }
    }
}

/// Load the CLAP plugin at `plugin_path`, read plugin metadata, and optionally scan for presets.
pub fn scan_library(plugin_path: &Path, scan_presets: bool) -> Result<ScannedLibrary> {
    let library = crate::plugin::library::PluginLibrary::load(plugin_path)?;
    let metadata = library.metadata()?;

    let presets = if scan_presets && let Ok(preset_discovery_factory) = library.preset_discovery_factory() {
        let metadata = preset_discovery_factory
            .metadata()
            .context("Could not get the preset discovery's provider descriptors")?;

        let mut index = Vec::new();
        for provider_metadata in metadata {
            let provider = preset_discovery_factory
                .create_provider(&provider_metadata)
                .with_context(|| format!("Could not create the provider with ID '{}'", provider_metadata.id))?;

            let declared_data = provider.declared_data();
            let mut presets = BTreeMap::new();
            for location in &declared_data.locations {
                presets.extend(provider.crawl_location(location).with_context(|| {
                    format!(
                        "Error occurred while crawling presets for the location '{}' with {} using provider '{}' with \
                         ID '{}'",
                        location.name, location.value, provider_metadata.name, provider_metadata.id,
                    )
                })?);
            }

            index.push(ScannedPresets {
                provider_id: provider_metadata.id,
                provider_name: provider_metadata.name,
                provider_vendor: provider_metadata.vendor,
                provider_version: provider_metadata.version,
                soundpacks: declared_data.soundpacks.clone(),
                presets: presets.into_iter().collect(),
            });
        }

        index
    } else {
        vec![]
    };

    Ok(ScannedLibrary {
        version: metadata.version,
        plugins: metadata.plugins,
        preset_providers: presets,
    })
}

/// Index all installed CLAP plugins by searching the standard directories. Returns a list of
/// paths to all found plugins, or an error if the directories could not be determined.
///
/// This does not load or validate the plugins in any way.
pub fn index_plugins() -> Result<Vec<PathBuf>> {
    let mut plugins = vec![];

    let directories = clap_directories().context("Could not find the CLAP plugin locations")?;
    for directory in directories {
        for clap_plugin_path in walk_clap_plugins(&directory) {
            plugins.push(clap_plugin_path.into_path());
        }
    }

    Ok(plugins)
}

/// Get the platform-specific CLAP directories. This takes `$CLAP_PATH` into account. Returns an
/// error if the paths could not be parsed correctly.
///
/// While not part of the specification, the Linux paths are also used on the BSDs.
#[cfg(all(target_family = "unix", not(target_os = "macos")))]
pub fn clap_directories() -> Result<Vec<PathBuf>> {
    let home_dir = std::env::var("HOME").context("'$HOME' is not set")?;

    let mut directories = clap_env_path_directories();
    directories.push(Path::new(&home_dir).join(".clap"));
    directories.push(PathBuf::from("/usr/lib/clap"));

    Ok(directories)
}

/// Get the platform-specific CLAP directories. This takes `$CLAP_PATH` into account. Returns an
/// error if the paths could not be parsed correctly.
#[cfg(target_os = "macos")]
pub fn clap_directories() -> Result<Vec<PathBuf>> {
    let home_dir = std::env::var("HOME").context("'$HOME' is not set")?;

    let mut directories = clap_env_path_directories();
    directories.push(Path::new(&home_dir).join("Library/Audio/Plug-Ins/CLAP"));
    directories.push(PathBuf::from("/Library/Audio/Plug-Ins/CLAP"));

    Ok(directories)
}

/// Get the platform-specific CLAP directories. This takes `$CLAP_PATH` into account. Returns an
/// error if the paths could not be parsed correctly.
#[cfg(windows)]
pub fn clap_directories() -> Result<Vec<PathBuf>> {
    let common_files = std::env::var("COMMONPROGRAMFILES").context("'$COMMONPROGRAMFILES' is not set")?;
    let local_appdata = std::env::var("LOCALAPPDATA").context("'$LOCALAPPDATA' is not set")?;

    // TODO: Does this work reliably? There are dedicated Win32 API functions for getting these
    //       directories, but I'd rather avoid adding a dependency just for that.
    let mut directories = clap_env_path_directories();
    directories.push(Path::new(&common_files).join("CLAP"));
    directories.push(Path::new(&local_appdata).join("Programs/Common/CLAP"));

    Ok(directories)
}

/// Parse `$CLAP_PATH` by splitting on on colons. This will return an empty Vec if the environment
/// variable is not set.
fn clap_env_path_directories() -> Vec<PathBuf> {
    std::env::var("CLAP_PATH")
        .map(|clap_path| clap_path.split(PATH_SEPARATOR).map(PathBuf::from).collect())
        .unwrap_or_else(|_| Vec::new())
}

/// Return an iterator over all `.clap` plugins under `directory`. These will be files on Linux and
/// Windows, and (bundle) directories on macOS.
fn walk_clap_plugins(directory: &Path) -> impl Iterator<Item = DirEntry> {
    WalkDir::new(directory)
        .min_depth(1)
        .follow_links(true)
        .same_file_system(false)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| is_clap_plugin(entry.path()))
}

fn is_clap_plugin(path: &Path) -> bool {
    if path.extension().is_some_and(|ext| ext == "clap") {
        return false;
    }

    let path = match std::fs::canonicalize(path) {
        Ok(path) => path,
        Err(_) => return false,
    };

    if cfg!(target_os = "macos") {
        path.is_dir()
    } else {
        path.is_file()
    }
}
