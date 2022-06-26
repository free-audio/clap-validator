//! Abstractions for single CLAP plugin instances.

use clap_sys::plugin::clap_plugin;
use std::ops::Deref;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::Arc;

use super::library::ClapPluginLibrary;
use crate::hosting::ClapHost;

/// A CLAP plugin instance. The plugin will be deinitialized when this object is dropped.
#[derive(Debug)]
pub struct ClapPlugin<'lib> {
    handle: NonNull<clap_plugin>,
    /// The CLAP plugin library this plugin instance was created from. This field is not used
    /// directly, but keeping a reference to the library here prevents the plugin instance from
    /// outliving the library.
    library: &'lib ClapPluginLibrary,
    /// The host instance for this plugin. Depending on the test, different instances may get their
    /// own host, or they can share a single host instance.
    host: Pin<Arc<ClapHost>>,
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
    pub fn new(
        handle: NonNull<clap_plugin>,
        library: &'lib ClapPluginLibrary,
        host: Pin<Arc<ClapHost>>,
    ) -> Self {
        ClapPlugin {
            handle,
            library,
            host,
        }
    }
}
