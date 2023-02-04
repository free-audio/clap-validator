//! Abstractions for single CLAP plugin instances for audio thread interactions.

use anyhow::Result;
use clap_sys::plugin::clap_plugin;
use clap_sys::process::{
    CLAP_PROCESS_CONTINUE, CLAP_PROCESS_CONTINUE_IF_NOT_QUIET, CLAP_PROCESS_ERROR,
    CLAP_PROCESS_SLEEP, CLAP_PROCESS_TAIL,
};
use std::marker::PhantomData;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::Arc;

use crate::plugin::host::InstanceState;
use crate::util::unsafe_clap_call;

use super::process::ProcessData;
use super::{assert_plugin_state_eq, assert_plugin_state_initialized};
use super::{Plugin, PluginStatus};
use crate::plugin::ext::Extension;

/// An audio thread equivalent to [`Plugin`]. This version only allows audio thread functions to be
/// called. It can be constructed using [`Plugin::on_audio_thread()`].
#[derive(Debug)]
pub struct PluginAudioThread<'a> {
    /// The plugin instance this audio thread belongs to. This is needed to ensure that the audio
    /// thread instance cannot outlive the plugin instance (which cannot outlive the plugin
    /// library). This `Plugin` also contains a reference to the plugin instance's state.
    plugin: &'a Plugin<'a>,
    /// To honor CLAP's thread safety guidelines, this audio thread abstraction cannot be shared
    /// with or sent to other threads.
    _send_sync_marker: PhantomData<*const ()>,
}

/// The equivalent of `clap_process_status`, minus the `CLAP_PROCESS_ERROR` value as this is already
/// treated as an error by `PluginAudioThread::process()`.
#[derive(Debug)]
pub enum ProcessStatus {
    Continue,
    ContinueIfNotQuiet,
    Tail,
    Sleep,
}

impl Drop for PluginAudioThread<'_> {
    fn drop(&mut self) {
        match self
            .state()
            .status
            .compare_exchange(PluginStatus::Processing, PluginStatus::Activated)
        {
            Ok(_) => self.stop_processing(),
            Err(PluginStatus::Activated) => (),
            Err(state) => panic!(
                "The plugin was in an invalid state '{state:?}' when the audio thread got \
                 dropped, this is a clap-validator bug"
            ),
        }
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

    /// Get the underlying `Plugin`'s [`InstanceState`] object.
    pub fn state(&self) -> &Pin<Arc<InstanceState>> {
        &self.plugin.state
    }

    /// Get the plugin's current initialization status.
    pub fn status(&self) -> PluginStatus {
        self.state().status.load()
    }

    /// Get the _audio thread_ extension abstraction for the extension `T`, if the plugin supports
    /// this extension. Returns `None` if it does not. The plugin needs to be initialized using
    /// [`init()`][Self::init()] before this may be called.
    //
    // TODO: Remove this unused attribute once we implement audio thread extensions
    #[allow(unused)]
    pub fn get_extension<T: Extension<&'a Self>>(&'a self) -> Option<T> {
        assert_plugin_state_initialized!(self);

        let plugin = self.as_ptr();
        let extension_ptr =
            unsafe_clap_call! { plugin=>get_extension(plugin, T::EXTENSION_ID.as_ptr()) };

        if extension_ptr.is_null() {
            None
        } else {
            Some(T::new(
                self,
                NonNull::new(extension_ptr as *mut T::Struct).unwrap(),
            ))
        }
    }

    /// Prepare for audio processing. Returns an error if the plugin returned `false`. See
    /// [plugin.h](https://github.com/free-audio/clap/blob/main/include/clap/plugin.h) for the
    /// preconditions.
    pub fn start_processing(&self) -> Result<()> {
        assert_plugin_state_eq!(self, PluginStatus::Activated);

        let plugin = self.as_ptr();
        if unsafe_clap_call! { plugin=>start_processing(plugin) } {
            self.state().status.store(PluginStatus::Processing);
            Ok(())
        } else {
            anyhow::bail!("'clap_plugin::start_processing()' returned false")
        }
    }

    /// Process audio. If the plugin returned either `CLAP_PROCESS_ERROR` or an unknown process
    /// status code, then this will return an error. See
    /// [plugin.h](https://github.com/free-audio/clap/blob/main/include/clap/plugin.h) for the
    /// preconditions.
    pub fn process(&self, process_data: &mut ProcessData) -> Result<ProcessStatus> {
        assert_plugin_state_eq!(self, PluginStatus::Processing);

        let plugin = self.as_ptr();
        let result = process_data.with_clap_process_data(|clap_process_data| {
            unsafe_clap_call! { plugin=>process(plugin, &clap_process_data) }
        });

        match result {
            CLAP_PROCESS_ERROR => anyhow::bail!(
                "The plugin returned 'CLAP_PROCESS_ERROR' from 'clap_plugin::process()'"
            ),
            CLAP_PROCESS_CONTINUE => Ok(ProcessStatus::Continue),
            CLAP_PROCESS_CONTINUE_IF_NOT_QUIET => Ok(ProcessStatus::ContinueIfNotQuiet),
            CLAP_PROCESS_TAIL => Ok(ProcessStatus::Tail),
            CLAP_PROCESS_SLEEP => Ok(ProcessStatus::Sleep),
            result => anyhow::bail!(
                "The plugin returned an unknown 'clap_process_status' value {result} from \
                 'clap_plugin::process()'"
            ),
        }
    }

    /// Stop processing audio. See
    /// [plugin.h](https://github.com/free-audio/clap/blob/main/include/clap/plugin.h) for the
    /// preconditions.
    pub fn stop_processing(&self) {
        assert_plugin_state_eq!(self, PluginStatus::Processing);

        let plugin = self.as_ptr();
        unsafe_clap_call! { plugin=>stop_processing(plugin) };

        self.state().status.store(PluginStatus::Activated);
    }
}
