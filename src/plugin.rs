//! Contains functions for loading CLAP plugins.

use anyhow::{Context, Result};
use clap_sys::entry::clap_plugin_entry;
use clap_sys::plugin_factory::{clap_plugin_factory, CLAP_PLUGIN_FACTORY_ID};
use serde::Serialize;
use std::{
    ffi::CString,
    path::{Path, PathBuf},
};

use crate::util;

/// A list of known CLAP plugins found on this system. See [`index()`].
#[derive(Debug)]
pub struct ClapPlugin {
    /// The plugin's library. Its entry point has already been initialized, and it will
    /// autoamtically be deinitialized when this object gets dropped.
    library: libloading::Library,
}

/// Metadata for a CLAP plugin library, which may contain multiple plugins.
#[derive(Debug, Serialize)]
pub struct ClapMetadata {
    pub version: (u32, u32, u32),
    pub plugins: Vec<ClapPluginMetadata>,
}

/// Metadata for a single plugin within a CLAP plugin library. See
/// [plugin.h](https://github.com/free-audio/clap/blob/main/include/clap/plugin.h) for a description
/// of the fields.
#[derive(Debug, Serialize)]
pub struct ClapPluginMetadata {
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    pub vendor: Option<String>,
    pub description: Option<String>,
    pub manual_url: Option<String>,
    pub support_url: Option<String>,
    pub features: Vec<String>,
}

impl Drop for ClapPlugin {
    fn drop(&mut self) {
        // The `ClapPlugin` only exists if `init()` returned true, so we ned to deinitialize the
        // plugin here
        let entry_point = get_clap_entry_point(&self.library)
            .expect("A ClapPlugin was constructed for a plugin with no entry point");
        unsafe { (entry_point.deinit)() };
    }
}

impl ClapPlugin {
    /// Load a CLAP plugin from a path to a `.clap` file or bundle. This will return an error if the
    /// plugin could not be loaded.
    pub fn load(path: impl AsRef<Path>) -> Result<ClapPlugin> {
        // NOTE: We'll always make sure `path` is either relative to the current directory or
        //       absolute. Otherwise the system libraries may be searched instead which would lead
        //       to unexpected behavior. Joining an absolute path to a relative directory gets you
        //       the absolute path, so this won't cause any issues.
        let path = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path);

        let library =
            unsafe { libloading::Library::new(&path) }.context("Could not load the library")?;

        // The entry point needs to be initialized before it can be used. It will be deinitialized
        // when the `ClapPlugin` object is dropped.
        let entry_point = get_clap_entry_point(&library)?;
        let path_cstring = CString::new(
            path.as_os_str()
                .to_str()
                .context("Path contains invalid UTF-8")?,
        )
        .context("Path contains null bytes")?;
        if !unsafe { (entry_point.init)(path_cstring.as_ptr()) } {
            anyhow::bail!("'clap_plugin_entry::init({path_cstring:?})' returned false");
        }

        Ok(ClapPlugin { library })
    }

    /// Get the metadata for all plugins stored in this plugin library. Most plugin libraries
    /// contain a single plugin, but this may return metadata for zero or more plugins.
    pub fn metadata(&self) -> Result<ClapMetadata> {
        let entry_point = get_clap_entry_point(&self.library)
            .expect("A ClapPlugin was constructed for a plugin with no entry point");
        let plugin_factory = unsafe { (entry_point.get_factory)(CLAP_PLUGIN_FACTORY_ID) }
            as *const clap_plugin_factory;
        // TODO: Should we log anything here? In theory not supporting the plugin factory is
        //       perfectly legal, but it's a bit weird
        if plugin_factory.is_null() {
            anyhow::bail!("The plugin does not support the 'clap_plugin_factory'");
        }

        let mut metadata = ClapMetadata {
            version: (
                entry_point.clap_version.major,
                entry_point.clap_version.minor,
                entry_point.clap_version.revision,
            ),
            plugins: Vec::new(),
        };
        let num_plugins = unsafe { ((*plugin_factory).get_plugin_count)(plugin_factory) };
        for i in 0..num_plugins {
            let descriptor =
                unsafe { ((*plugin_factory).get_plugin_descriptor)(plugin_factory, i) };
            if descriptor.is_null() {
                anyhow::bail!("The plugin returned a null plugin descriptor for plugin index {i} (expected {num_plugins} total plugins)");
            }

            metadata.plugins.push(ClapPluginMetadata {
                id: unsafe { util::cstr_ptr_to_string((*descriptor).id) }
                    .context("The plugin's 'id' pointer was null")?,
                name: unsafe { util::cstr_ptr_to_string((*descriptor).name) }
                    .context("The plugin's 'id' pointer was null")?,
                version: unsafe { util::cstr_ptr_to_string((*descriptor).version) },
                vendor: unsafe { util::cstr_ptr_to_string((*descriptor).vendor) },
                description: unsafe { util::cstr_ptr_to_string((*descriptor).description) },
                manual_url: unsafe { util::cstr_ptr_to_string((*descriptor).manual_url) },
                support_url: unsafe { util::cstr_ptr_to_string((*descriptor).support_url) },
                features: unsafe { util::cstr_array_to_vec((*descriptor).features) }
                    .context("The plugin's 'features' were malformed")?,
            })
        }

        Ok(metadata)
    }
}

/// Get a plugin's entry point.
fn get_clap_entry_point(library: &libloading::Library) -> Result<&clap_plugin_entry> {
    let entry_point: libloading::Symbol<*const clap_plugin_entry> =
        unsafe { library.get(b"clap_entry") }
            .context("The library does not expose a 'clap_entry' symbol")?;
    if entry_point.is_null() {
        anyhow::bail!("'clap_entry' is a null pointer");
    }

    Ok(unsafe { &**entry_point })
}
