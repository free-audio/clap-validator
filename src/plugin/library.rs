//! Interactions with CLAP plugin libraries, which may contain multiple plugins.

use anyhow::{Context, Result};
use clap_sys::entry::clap_plugin_entry;
use clap_sys::factory::plugin_factory::{clap_plugin_factory, CLAP_PLUGIN_FACTORY_ID};
use clap_sys::version::clap_version;
use serde::Serialize;
use std::collections::HashSet;
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::instance::Plugin;
use crate::host::Host;
use crate::util::{self, unsafe_clap_call};

/// A CLAP plugin library built from a CLAP plugin's entry point. This can be used to iterate over
/// all plugins exposed by the library and to initialize plugins.
#[derive(Debug)]
pub struct PluginLibrary {
    /// The path to this plugin library. On macOS, this points to the bundle's root instead of the
    /// library contained within the bundle.
    library_path: PathBuf,
    /// The plugin's library. Its entry point has already been initialized, and it will
    /// autoamtically be deinitialized when this object gets dropped.
    library: libloading::Library,
}

/// Metadata for a CLAP plugin library, which may contain multiple plugins.
#[derive(Debug, Serialize)]
pub struct PluginLibraryMetadata {
    pub version: (u32, u32, u32),
    pub plugins: Vec<PluginMetadata>,
}

/// Metadata for a single plugin within a CLAP plugin library. See
/// [plugin.h](https://github.com/free-audio/clap/blob/main/include/clap/plugin.h) for a description
/// of the fields.
#[derive(Debug, Serialize)]
pub struct PluginMetadata {
    pub id: String,
    pub name: String,
    pub version: Option<String>,
    pub vendor: Option<String>,
    pub description: Option<String>,
    pub manual_url: Option<String>,
    pub support_url: Option<String>,
    pub features: Vec<String>,
}

impl Drop for PluginLibrary {
    fn drop(&mut self) {
        // The `Plugin` only exists if `init()` returned true, so we ned to deinitialize the
        // plugin here
        let entry_point = get_clap_entry_point(&self.library)
            .expect("A Plugin was constructed for a plugin with no entry point");
        unsafe_clap_call! { entry_point=>deinit() };
    }
}

impl PluginLibrary {
    /// Load a CLAP plugin from a path to a `.clap` file or bundle. This will return an error if the
    /// plugin could not be loaded.
    pub fn load(path: impl AsRef<Path>) -> Result<PluginLibrary> {
        Self::load_with(path, |path| {
            unsafe { libloading::Library::new(path) }.context("Could not load the plugin library")
        })
    }

    /// The same as [`load()`][`Self::load()`], but with a custom library loading function. Useful
    /// for testing different `dlopen()` options.
    pub fn load_with(
        path: impl AsRef<Path>,
        load: impl FnOnce(&Path) -> Result<libloading::Library>,
    ) -> Result<PluginLibrary> {
        // NOTE: We'll always make sure `path` is either relative to the current directory or
        //       absolute. Otherwise the system libraries may be searched instead which would lead
        //       to unexpected behavior. Joining an absolute path to a relative directory gets you
        //       the absolute path, so this won't cause any issues.
        let path = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path);

        // NOTE: Apple says you can dlopen() bundles. This is a lie.
        #[cfg(target_os = "macos")]
        let path = {
            use core_foundation::bundle::CFBundle;
            use core_foundation::url::CFURL;

            let bundle =
                CFBundle::new(CFURL::from_path(path, true).context("Could not create CFURL")?)
                    .context("Could not open bundle")?;
            let executable = bundle
                .executable_url()
                .context("Could not get executable URL within bundle")?;

            executable
                .to_path()
                .context("Could not convert bundle executable path")?
        };
        let library = load(&path)?;

        // The entry point needs to be initialized before it can be used. It will be deinitialized
        // when the `Plugin` object is dropped.
        let entry_point = get_clap_entry_point(&library)?;
        let path_cstring = CString::new(
            path.as_os_str()
                .to_str()
                .context("Path contains invalid UTF-8")?,
        )
        .context("Path contains null bytes")?;
        if !unsafe_clap_call! { entry_point=>init(path_cstring.as_ptr()) } {
            anyhow::bail!("'clap_plugin_entry::init({path_cstring:?})' returned false");
        }

        Ok(PluginLibrary {
            library_path: path,
            library,
        })
    }

    pub fn library_path(&self) -> &Path {
        &self.library_path
    }

    /// Get the metadata for all plugins stored in this plugin library. Most plugin libraries
    /// contain a single plugin, but this may return metadata for zero or more plugins.
    pub fn metadata(&self) -> Result<PluginLibraryMetadata> {
        let entry_point = get_clap_entry_point(&self.library)
            .expect("A Plugin was constructed for a plugin with no entry point");
        let plugin_factory = unsafe_clap_call! { entry_point=>get_factory(CLAP_PLUGIN_FACTORY_ID.as_ptr()) }
            as *const clap_plugin_factory;
        // TODO: Should we log anything here? In theory not supporting the plugin factory is
        //       perfectly legal, but it's a bit weird
        if plugin_factory.is_null() {
            anyhow::bail!("The plugin does not support the 'clap_plugin_factory'");
        }

        let mut metadata = PluginLibraryMetadata {
            version: (
                entry_point.clap_version.major,
                entry_point.clap_version.minor,
                entry_point.clap_version.revision,
            ),
            plugins: Vec::new(),
        };
        let num_plugins = unsafe_clap_call! { plugin_factory=>get_plugin_count(plugin_factory) };
        for i in 0..num_plugins {
            let descriptor =
                unsafe_clap_call! { plugin_factory=>get_plugin_descriptor(plugin_factory, i) };
            if descriptor.is_null() {
                anyhow::bail!(
                    "The plugin returned a null plugin descriptor for plugin index {i} (expected \
                     {num_plugins} total plugins)"
                );
            }

            // Empty strings should be treated as missing values in some cases
            let handle_empty_string = |option: Option<String>| match option {
                Some(s) if s.is_empty() => None,
                option => option,
            };

            metadata.plugins.push(PluginMetadata {
                id: unsafe { util::cstr_ptr_to_string((*descriptor).id)? }
                    .context("The plugin's 'id' pointer was null")?,
                name: unsafe { util::cstr_ptr_to_string((*descriptor).name)? }
                    .context("The plugin's 'id' pointer was null")?,
                version: handle_empty_string(unsafe {
                    util::cstr_ptr_to_string((*descriptor).version)?
                }),
                vendor: handle_empty_string(unsafe {
                    util::cstr_ptr_to_string((*descriptor).vendor)?
                }),
                description: handle_empty_string(unsafe {
                    util::cstr_ptr_to_string((*descriptor).description)?
                }),
                manual_url: handle_empty_string(unsafe {
                    util::cstr_ptr_to_string((*descriptor).manual_url)?
                }),
                support_url: handle_empty_string(unsafe {
                    util::cstr_ptr_to_string((*descriptor).support_url)?
                }),
                features: unsafe { util::cstr_array_to_vec((*descriptor).features)? }
                    .context("The plugin's 'features' were malformed")?,
            })
        }

        // As a sanity check we'll make sure there are no duplicate plugin IDs here
        let unique_plugin_ids: HashSet<&str> = metadata
            .plugins
            .iter()
            .map(|plugin_metadata| plugin_metadata.id.as_str())
            .collect();
        if unique_plugin_ids.len() != metadata.plugins.len() {
            anyhow::bail!("The plugin's factory contains multiple entries for the same plugin ID.");
        }

        Ok(metadata)
    }

    /// Returns whether or not a factory with the specified ID exists. This is used in a test to
    /// assert that querying a factory with a non-existent ID returns a null pointer instead of
    /// always returning the plugin factory.
    pub fn factory_exists(&self, factory_id: &str) -> bool {
        let factory_id_cstring =
            CString::new(factory_id).expect("The factory ID contained internal null bytes");

        let entry_point = get_clap_entry_point(&self.library)
            .expect("A Plugin was constructed for a plugin with no entry point");
        let factory_pointer =
            unsafe_clap_call! { entry_point=>get_factory(factory_id_cstring.as_ptr()) };

        !factory_pointer.is_null()
    }

    /// Try to create the plugin with the given ID, and using the provided host instance. The plugin
    /// IDs supported by this plugin library can be found by calling
    /// [`metadata()`][Self::metadata()]. The returned plugin has not yet been initialized, and
    /// `destroy()` will be called automatically when the object is dropped.
    pub fn create_plugin(&self, id: &str, host: Arc<Host>) -> Result<Plugin> {
        let entry_point = get_clap_entry_point(&self.library)
            .expect("A Plugin was constructed for a plugin with no entry point");
        let plugin_factory = unsafe_clap_call! { entry_point=>get_factory(CLAP_PLUGIN_FACTORY_ID.as_ptr()) }
            as *const clap_plugin_factory;
        if plugin_factory.is_null() {
            anyhow::bail!("The plugin does not support the 'clap_plugin_factory'");
        }

        let id_cstring = CString::new(id).context("Plugin ID contained null bytes")?;
        Plugin::new(self, host, unsafe { &*plugin_factory }, &id_cstring)
    }
}

impl PluginLibraryMetadata {
    /// Get the CLAP version representation for this plugin library.
    pub fn clap_version(&self) -> clap_version {
        clap_version {
            major: self.version.0,
            minor: self.version.1,
            revision: self.version.2,
        }
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
