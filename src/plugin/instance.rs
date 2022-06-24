//! Abstractions for single CLAP plugin instances.

use clap_sys::plugin::clap_plugin;
use std::ops::Deref;
use std::ptr::NonNull;

use super::library::ClapPluginLibrary;

/// A CLAP plugin instance. The plugin will be deinitialized when this object is dropped.
#[derive(Debug)]
pub struct ClapPlugin<'lib> {
    handle: NonNull<clap_plugin>,
    /// The CLAP plugin library this plugin instance was created from. The plugin's host instance
    /// also comes from here
    library: &'lib ClapPluginLibrary,
}

impl Drop for ClapPlugin<'_> {
    fn drop(&mut self) {
        unsafe { (self.handle.as_ref().destroy)(self.handle.as_ptr()) };
    }
}

impl Deref for ClapPlugin<'_> {
    type Target = clap_plugin;

    fn deref(&self) -> &Self::Target {
        unsafe { self.handle.as_ref() }
    }
}

impl<'lib> ClapPlugin<'lib> {
    pub fn new(handle: NonNull<clap_plugin>, library: &'lib ClapPluginLibrary) -> Self {
        ClapPlugin { handle, library }
    }
}
