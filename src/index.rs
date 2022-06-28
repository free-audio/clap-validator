//! Utilities and data structures for indexing plugins.

use anyhow::{Context, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use walkdir::{DirEntry, WalkDir};

use crate::plugin::library::{PluginLibrary, PluginLibraryMetadata};

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
