//! Abstractions for single CLAP plugin instances for audio thread interactions.

use clap_sys::plugin::clap_plugin;
use std::marker::PhantomData;
use std::ops::Deref;
use std::ptr::NonNull;

use super::ext::Extension;
use super::instance::Plugin;

/// An audio thread equivalent to [`Plugin`]. This version only allows audio thread functions to be
/// called. It can be constructed using [`Plugin::audio_thread()`].
#[derive(Debug)]
pub struct PluginAudioThread<'a> {
    /// The plugin instance this audio thread belongs to. This is needed to ensure that the audio
    /// thread instance cannot outlive the plugin instance (which cannot outlive the plugin
    /// library).
    plugin: &'a Plugin<'a>,
    /// To honor CLAP's thread safety guidelines, this audio thread abstraction cannot be shared
    /// with or sent to other threads.
    _send_sync_marker: PhantomData<*const ()>,
}

/// This allows methods from the CLAP plugin to be called directly independently of any
/// abstractions. All of the thread guarentees are lost when interacting with the plugin this way,
/// but that is not a problem as the function pointers are marked unsafe anyways.
impl Deref for PluginAudioThread<'_> {
    type Target = clap_plugin;

    fn deref(&self) -> &Self::Target {
        self.plugin.deref()
    }
}

impl<'a> PluginAudioThread<'a> {
    pub fn new(plugin: &'a Plugin) -> Self {
        PluginAudioThread {
            plugin,
            _send_sync_marker: PhantomData,
        }
    }

    /// Get the raw pointer to the `clap_plugin` instance.
    pub fn as_ptr(&self) -> *const clap_plugin {
        self.plugin.as_ptr()
    }

    /// Get the _audio thread_ extension abstraction for the extension `T`, if the plugin supports
    /// this extension. Returns `None` if it does not. The plugin needs to be initialized using
    /// [`init()`][Self::init()] before this may be called.
    pub fn get_extension<T: Extension<&'a Self>>(&'a self) -> Option<T> {
        let extension_ptr = unsafe { (self.plugin.get_extension)(self.as_ptr(), T::EXTENSION_ID) };

        if extension_ptr.is_null() {
            None
        } else {
            Some(T::new(
                self,
                NonNull::new(extension_ptr as *mut T::Struct).unwrap(),
            ))
        }
    }

    // TODO: Add abstraction functions
}
