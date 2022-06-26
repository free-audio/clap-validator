//! Abstractions for interacting with the `audio-ports` extension.

use std::os::raw::c_char;
use std::ptr::NonNull;

use clap_sys::ext::audio_ports::{clap_plugin_audio_ports, CLAP_EXT_AUDIO_PORTS};

use crate::plugin::instance::Plugin;

use super::Extension;

/// Abstraction for the `audio-ports` extension covering the main thread functionality.
#[derive(Debug)]
pub struct AudioPorts<'a> {
    plugin: &'a Plugin<'a>,
    audio_ports: NonNull<clap_plugin_audio_ports>,
}

impl<'a> Extension<&'a Plugin<'a>> for AudioPorts<'a> {
    const EXTENSION_ID: *const c_char = CLAP_EXT_AUDIO_PORTS;

    type Struct = clap_plugin_audio_ports;

    fn new(plugin: &'a Plugin<'a>, extension_struct: NonNull<Self::Struct>) -> Self {
        Self {
            plugin,
            audio_ports: extension_struct,
        }
    }
}
