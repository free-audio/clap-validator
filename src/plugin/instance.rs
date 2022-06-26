//! Abstractions for single CLAP plugin instances.

use clap_sys::plugin::clap_plugin;
use std::marker::PhantomData;
use std::ops::Deref;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::Arc;

use super::library::PluginLibrary;
use crate::hosting::ClapHost;

/// A CLAP plugin instance. The plugin will be deinitialized when this object is dropped. All
/// functions here are callable only from the main thread. Use the
/// [`audio_thread()`][Self::audio_thread()] method to spawn an audio thread.
#[derive(Debug)]
pub struct Plugin<'lib> {
    handle: NonNull<clap_plugin>,
    /// The CLAP plugin library this plugin instance was created from. This field is not used
    /// directly, but keeping a reference to the library here prevents the plugin instance from
    /// outliving the library.
    _library: &'lib PluginLibrary,
    /// The host instance for this plugin. Depending on the test, different instances may get their
    /// own host, or they can share a single host instance.
    _host: Pin<Arc<ClapHost>>,
    /// To honor CLAP's thread safety guidelines, the thread this object was created from is
    /// designated the 'main thread', and this object cannot be shared with other threads. The
    /// [`audio_thread()`][Self::audio_thread()] method spawns an audio thread that is able to call
    /// the plugin's audio thread functions.
    _send_sync_marker: PhantomData<*const ()>,
}

impl Drop for Plugin<'_> {
    fn drop(&mut self) {
        unsafe { (self.handle.as_ref().destroy)(self.handle.as_ptr()) };
    }
}

/// This allows methods from the CLAP plugin to be called directly independently of any
/// abstractions. All of the thread guarentees are lost when interacting with the plugin this way,
/// but that is not a problem as the function pointers are marked unsafe anyways.
impl Deref for Plugin<'_> {
    type Target = clap_plugin;

    fn deref(&self) -> &Self::Target {
        unsafe { self.handle.as_ref() }
    }
}

impl<'lib> Plugin<'lib> {
    pub fn new(
        handle: NonNull<clap_plugin>,
        library: &'lib PluginLibrary,
        host: Pin<Arc<ClapHost>>,
    ) -> Self {
        Plugin {
            handle,
            _library: library,
            _host: host,
            _send_sync_marker: PhantomData,
        }
    }
}
