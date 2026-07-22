//! Abstractions for the different extensions. The extension `Foo` comes with a `Foo` and a
//! `FooAudioThread` struct. The former contains functions that can be called from the main thread,
//! while the latter contains functions that can be called from the audio thread.

use std::ffi::CStr;
use std::ptr::NonNull;

pub mod ambisonic;
pub mod audio_ports;
pub mod audio_ports_activation;
pub mod audio_ports_config;
pub mod configurable_audio_ports;
pub mod latency;
pub mod note_ports;
pub mod params;
pub mod preset_load;
pub mod render;
pub mod state;
pub mod surround;
pub mod tail;
pub mod thread_pool;
pub mod voice_info;

/// An abstraction for a CLAP plugin extension.
pub trait Extension {
    /// The list of C-string IDs for the extension.
    const IDS: &'static [&'static CStr];

    /// The plugin type (`Plugin` for main-thread, `PluginShared` for shared, `PluginAudioThread` for audio-thread) for which this extension is implemented.
    type Plugin;
    /// The type of the C-struct for the extension.
    type Struct;

    /// Construct the extension for the plugin type `P`. This allows the abstraction to be limited
    /// to only work with the main thread `&Plugin` or the audio thread `&PluginAudioThread`.
    ///
    /// # Safety
    /// The extension struct pointer must be a valid pointer to the correct extension struct for
    /// the plugin instance and given `IDS`.
    unsafe fn new(plugin: Self::Plugin, extension_struct: NonNull<Self::Struct>) -> Self;
}
