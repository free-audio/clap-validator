//! Abstractions for the different extensions. The extension `Foo` comes with a `Foo` and a
//! `FooAudioThread` struct. The former contains functions that can be called from the main thread,
//! while the latter contains functions that can be called from the audio thread.

use std::ffi::CStr;
use std::ptr::NonNull;

pub mod audio_ports;
pub mod note_ports;
pub mod params;

/// An abstraction for a CLAP plugin extension. `P` here is the plugin type. In practice, this is
/// either `Plugin` or `PluginAudioThread`. Abstractions for main thread functions will implement
/// this trait for `Plugin`, and abstractions for audio thread functions will implement this trait
/// for `PluginAudioThread`.
pub trait Extension<P> {
    /// The C-string ID for the extension.
    const EXTENSION_ID: &'static CStr;

    /// The type of the C-struct for the extension.
    type Struct;

    /// Construct the extension for the plugin type `P`. This allows the abstraction to be limited
    /// to only work with the main thread `&Plugin` or the audio thread `&PluginAudioThread`.
    fn new(plugin: P, extension_struct: NonNull<Self::Struct>) -> Self;
}
