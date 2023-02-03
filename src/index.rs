//! Utilities and data structures for indexing plugins and presets.

use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use walkdir::{DirEntry, WalkDir};

use crate::plugin::library::{PluginLibrary, PluginLibraryMetadata};
use crate::plugin::preset_discovery::{PresetFile, Soundpack};

/// The separator for path environment variables.
#[cfg(unix)]
const PATH_SEPARATOR: char = ':';
/// The separator for path environment variables.
#[cfg(windows)]
const PATH_SEPARATOR: char = ';';

/// A map containing metadata for all CLAP plugins found on this system. Each plugin path in the map
/// contains zero or more plugins. See [`index()`].
///
/// Uses a `BTreeMap` purely so the order is stable.
#[derive(Debug, Serialize)]
pub struct Index(pub BTreeMap<PathBuf, PluginLibraryMetadata>);

/// Build an index of all CLAP plugins on this system. This finds all `.clap` files as specified in
/// [entry.h](https://github.com/free-audio/clap/blob/main/include/clap/entry.h), and lists all
/// plugins contained within those files. If a `.clap` file was found during the scan that could not
/// be read, then a warning will be printed.
pub fn index() -> Index {
    let mut index = Index(BTreeMap::new());
    let directories = match clap_directories() {
        Ok(directories) => directories,
        Err(err) => {
            log::error!("Could not find the CLAP plugin locations: {err:#}");
            return index;
        }
    };

    for directory in directories {
        for clap_plugin_path in walk_clap_plugins(&directory) {
            let metadata = PluginLibrary::load(clap_plugin_path.path())
                .with_context(|| format!("Could not load '{}'", clap_plugin_path.path().display()))
                .and_then(|plugin| {
                    plugin.metadata().with_context(|| {
                        format!(
                            "Could not fetch plugin metadata for '{}'",
                            clap_plugin_path.path().display()
                        )
                    })
                });

            match metadata {
                Ok(metadata) => {
                    index.0.insert(clap_plugin_path.into_path(), metadata);
                }
                Err(err) => log::error!("{err:#}"),
            }
        }
    }

    index
}

/// A map containing metadata for all presets supported by a set of `.clap` plugin library files.
///
/// Uses a `BTreeMap` purely so the order is stable.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct PresetIndex {
    /// All successfully crawled `.clap` files. If an error occurred, it will be added to `failed`
    /// instead.
    pub success: BTreeMap<PathBuf, Vec<ProviderPresets>>,
    pub failed: BTreeMap<PathBuf, String>,
}

/// Preset information declared by a preset provider.
#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "kebab-case")]
pub struct ProviderPresets {
    /// The preset provider's name.
    provider_name: String,
    /// The preset provider's vendor.
    provider_vendor: Option<String>,
    // All sound packs declared by the plugin.
    soundpacks: Vec<Soundpack>,
    // All presets declared by the plugin, indexed by URI.
    presets: BTreeMap<String, PresetFile>,
}

/// Index the presets for one or more plugins. [`index()`] can be used to build a list of all
/// installed CLAP plugins. Plugins that
pub fn index_presets<I, P>(plugin_paths: I, skip_unsupported: bool) -> Result<PresetIndex>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let mut index = PresetIndex::default();

    for path in plugin_paths {
        let path = path.as_ref();
        let library = crate::plugin::library::PluginLibrary::load(path)
            .with_context(|| format!("Could not load '{}'", path.display()))?;

        let preset_discovery_factory = library.preset_discovery_factory().with_context(|| {
            format!(
                "Could not get the preset discovery factory for '{}",
                path.display()
            )
        });
        if preset_discovery_factory.is_err() && skip_unsupported {
            continue;
        }
        let preset_discovery_factory = preset_discovery_factory?;

        let result = preset_discovery_factory
            .metadata()
            .context("Could not get the preset discovery's provider descriptors")
            .and_then(|metadata| {
                let mut provider_results = Vec::new();
                for provider_metadata in metadata {
                    let provider = preset_discovery_factory
                        .create_provider(&provider_metadata)
                        .with_context(|| {
                            format!(
                                "Could not create the provider with ID '{}'",
                                provider_metadata.id
                            )
                        })?;

                    let declared_data = provider.declared_data();
                    let mut presets = BTreeMap::new();
                    for location in &declared_data.locations {
                        presets.extend(provider.crawl_location(location).with_context(|| {
                            format!(
                                "Error occurred while crawling presets for the location '{}' with \
                                 URI '{}' using provider '{}' with ID '{}'",
                                location.name,
                                location.uri.to_uri(),
                                provider_metadata.name,
                                provider_metadata.id,
                            )
                        })?);
                    }

                    provider_results.push(ProviderPresets {
                        provider_name: provider_metadata.name,
                        provider_vendor: provider_metadata.vendor,
                        soundpacks: declared_data.soundpacks.clone(),
                        presets,
                    });
                }

                Ok(provider_results)
            });

        match result {
            Ok(provider_results) => {
                index.success.insert(path.to_owned(), provider_results);
            }
            Err(err) => {
                index.failed.insert(path.to_owned(), format!("{err:#}"));
            }
        }
    }

    Ok(index)
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
    let common_files =
        std::env::var("COMMONPROGRAMFILES").context("'$COMMONPROGRAMFILES' is not set")?;
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
        // Only consider valid `.clap` files or bundles. We'll need to follow symlinks as part of
        // that check.
        .filter(|entry| match entry.file_name().to_str() {
            #[cfg(not(target_os = "macos"))]
            Some(file_name) => {
                file_name.ends_with(".clap")
                    && std::fs::canonicalize(entry.path())
                        .map(|path| path.is_file())
                        .unwrap_or(false)
            }
            #[cfg(target_os = "macos")]
            Some(file_name) => {
                file_name.ends_with(".clap")
                    && std::fs::canonicalize(entry.path())
                        .map(|path| path.is_dir())
                        .unwrap_or(false)
            }
            None => false,
        })
}
