//! Contains functions for loading CLAP plugins.

use anyhow::{Context, Result};
use clap_sys::entry::clap_plugin_entry;
use serde::Serialize;
use std::{
    ffi::CString,
    path::{Path, PathBuf},
};

/// A list of known CLAP plugins found on this system. See [`index()`].
#[derive(Serialize)]
pub struct ClapPlugin {
    // TODO: Store metadata for the contained plugins
    //
    /// The plugin's library. Its entry point has already been initialized, and it will
    /// autoamtically be deinitialized when this object gets dropped.
    #[serde(skip)]
    library: libloading::Library,
}

impl Drop for ClapPlugin {
    fn drop(&mut self) {
        // The `ClapPlugin` only exists if `init()` returned true, so we ned to deinitialize the
        // plugin here
        unsafe {
            (get_clap_entry_point(&self.library)
                .expect("A ClapPlugin was constructed for a plugin with no entry point")
                .deinit)()
        };
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
        // whe nthe `ClapPlugin` object is dropped.
        let path_cstring = CString::new(
            path.as_os_str()
                .to_str()
                .context("Path contains invalid UTF-8")?,
        )
        .context("Path contains null bytes")?;
        let entry_point = get_clap_entry_point(&library)?;
        if !unsafe { (entry_point.init)(path_cstring.as_ptr()) } {
            anyhow::bail!("clap_plugin_entry::init({path_cstring:?}) returned false");
        }

        Ok(ClapPlugin { library })
    }
}

/// Get a plugin's entry point.
fn get_clap_entry_point(
    library: &libloading::Library,
) -> Result<libloading::Symbol<'_, clap_plugin_entry>> {
    unsafe { library.get(b"clap_entry") }
        .context("The library does not expose a 'clap_entry' symbol")
}
