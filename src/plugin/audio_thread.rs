//! Abstractions for single CLAP plugin instances for audio thread interactions.

use anyhow::Result;
use clap_sys::plugin::clap_plugin;
use clap_sys::process::{
    CLAP_PROCESS_CONTINUE, CLAP_PROCESS_CONTINUE_IF_NOT_QUIET, CLAP_PROCESS_ERROR,
    CLAP_PROCESS_SLEEP, CLAP_PROCESS_TAIL,
};
use std::marker::PhantomData;
use std::ops::Deref;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::Arc;

use crate::host::InstanceState;
use crate::util::unsafe_clap_call;

use self::process::ProcessData;
use super::ext::Extension;
use super::instance::{Plugin, PluginState};

pub mod process;

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

/// This allows methods from the CLAP plugin to be called directly independently of any
/// abstractions. All of the thread guarentees are lost when interacting with the plugin this way,
/// but that is not a problem as the function pointers are marked unsafe anyways.
impl Deref for PluginAudioThread<'_> {
    type Target = clap_plugin;

    fn deref(&self) -> &Self::Target {
        self.plugin.deref()
    }
}

impl Drop for PluginAudioThread<'_> {
    fn drop(&mut self) {
        match self
            .host_instance()
            .state
            .compare_exchange(PluginState::Processing, PluginState::Activated)
        {
            Ok(_) => unsafe_clap_call! { self.plugin=>stop_processing(self.plugin.as_ptr()) },
            Err(PluginState::Activated) => (),
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
    pub fn host_instance(&self) -> &Pin<Arc<InstanceState>> {
        &self.plugin.host_instance
    }

    /// Get the _audio thread_ extension abstraction for the extension `T`, if the plugin supports
    /// this extension. Returns `None` if it does not. The plugin needs to be initialized using
    /// [`init()`][Self::init()] before this may be called.
    //
    // TODO: Remove this unused attribute once we implement audio thread extensions:w
    #[allow(unused)]
    pub fn get_extension<T: Extension<&'a Self>>(&'a self) -> Option<T> {
        let extension_ptr = unsafe_clap_call! { self.plugin=>get_extension(self.as_ptr(), T::EXTENSION_ID.as_ptr()) };

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
        match self
            .host_instance()
            .state
            .compare_exchange(PluginState::Activated, PluginState::Processing)
        {
            Ok(_) => (),
            Err(PluginState::Processing) => anyhow::bail!(
                "Cannot start processing for a plugin that's already processing audio."
            ),
            Err(state) => panic!(
                "The plugin was in an invalid state '{state:?}' when trying to start processing, \
                 this is a clap-validator bug"
            ),
        }

        if unsafe_clap_call! { self.plugin=>start_processing(self.as_ptr()) } {
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
        let result = process_data.with_clap_process_data(|clap_process_data| {
            unsafe_clap_call! {
                self.plugin=>process(self.as_ptr(), &clap_process_data)
            }
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
    pub fn stop_processing(&self) -> Result<()> {
        match self
            .host_instance()
            .state
            .compare_exchange(PluginState::Processing, PluginState::Activated)
        {
            Ok(_) => (),
            Err(PluginState::Activated) => anyhow::bail!(
                "Cannot stop processing for a plugin that's currently not processing audio."
            ),
            Err(state) => panic!(
                "The plugin was in an invalid state '{state:?}' when trying to stop processing, \
                 this is a clap-validator bug"
            ),
        }

        unsafe_clap_call! { self.plugin=>stop_processing(self.as_ptr()) };

        Ok(())
    }
}
