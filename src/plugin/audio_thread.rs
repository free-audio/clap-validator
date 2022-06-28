//! Abstractions for single CLAP plugin instances for audio thread interactions.

use anyhow::Result;
use clap_sys::plugin::clap_plugin;
use clap_sys::process::{
    CLAP_PROCESS_CONTINUE, CLAP_PROCESS_CONTINUE_IF_NOT_QUIET, CLAP_PROCESS_ERROR,
    CLAP_PROCESS_TAIL,
};
use std::marker::PhantomData;
use std::ops::Deref;
use std::ptr::NonNull;

use self::process::ProcessData;
use super::ext::Extension;
use super::instance::Plugin;

pub mod process;

/// An audio thread equivalent to [`Plugin`]. This version only allows audio thread functions to be
/// called. It can be constructed using [`Plugin::on_audio_thread()`].
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
    //
    // TODO: Remove this unused attribute once we implement audio thread extensions:w
    #[allow(unused)]
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

    /// Prepare for audio processing. Returns an error if the plugin returned `false`. See
    /// [plugin.h](https://github.com/free-audio/clap/blob/main/include/clap/plugin.h) for the
    /// preconditions.
    pub fn start_processing(&self) -> Result<()> {
        if unsafe { (self.plugin.start_processing)(self.as_ptr()) } {
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
        let result = process_data.with_clap_process_data(|clap_process_data| unsafe {
            (self.plugin.process)(self.as_ptr(), &clap_process_data)
        });

        match result {
            CLAP_PROCESS_ERROR => anyhow::bail!(
                "The plugin returned 'CLAP_PROCESS_ERROR' from 'clap_plugin::process()'"
            ),
            CLAP_PROCESS_CONTINUE => Ok(ProcessStatus::Continue),
            CLAP_PROCESS_CONTINUE_IF_NOT_QUIET => Ok(ProcessStatus::ContinueIfNotQuiet),
            CLAP_PROCESS_TAIL => Ok(ProcessStatus::Tail),
            result => anyhow::bail!(
                "The plugin returned an unknown 'clap_process_status' value {result} from 'clap_plugin::process()'"
            ),
        }
    }

    /// Stop processing audio. See
    /// [plugin.h](https://github.com/free-audio/clap/blob/main/include/clap/plugin.h) for the
    /// preconditions.
    pub fn stop_processing(&self) {
        unsafe { (self.plugin.stop_processing)(self.as_ptr()) };
    }
}
