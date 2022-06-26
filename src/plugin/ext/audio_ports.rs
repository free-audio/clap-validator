//! Abstractions for interacting with the `audio-ports` extension.

use anyhow::Result;
use std::os::raw::c_char;
use std::ptr::NonNull;

use clap_sys::ext::audio_ports::{
    clap_audio_port_info, clap_plugin_audio_ports, CLAP_EXT_AUDIO_PORTS,
};

use crate::plugin::instance::Plugin;

use super::Extension;

/// Abstraction for the `audio-ports` extension covering the main thread functionality.
#[derive(Debug)]
pub struct AudioPorts<'a> {
    plugin: &'a Plugin<'a>,
    audio_ports: NonNull<clap_plugin_audio_ports>,
}

/// The audio port configuration for a plugin.
#[derive(Debug, Default)]
pub struct AudioPortConfig {
    /// Configuration for the plugin's input audio ports.
    pub inputs: Vec<AudioPort>,
    /// Configuration for the plugin's output audio ports.
    pub outputs: Vec<AudioPort>,
}

/// The configuration for a single audio port.
#[derive(Debug)]
pub struct AudioPort {
    /// The number of channels for an audio port.
    pub num_channels: u32,
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

impl AudioPorts<'_> {
    /// Get the audio port configuration for this plugin.
    pub fn config(&self) -> Result<AudioPortConfig> {
        let mut config = AudioPortConfig::default();

        let audio_ports = unsafe { self.audio_ports.as_ref() };
        let num_inputs = unsafe { (audio_ports.count)(&**self.plugin, true) };
        let num_outputs = unsafe { (audio_ports.count)(&**self.plugin, false) };

        for i in 0..num_inputs {
            let mut info: clap_audio_port_info = unsafe { std::mem::zeroed() };
            let success = unsafe { (audio_ports.get)(&**self.plugin, 0, true, &mut info) };
            if !success {
                anyhow::bail!("Plugin returned an error when querying input port {i} ({num_inputs} total input ports)");
            }

            // TODO: Test whether the channel count matches the port type
            config.inputs.push(AudioPort {
                num_channels: info.channel_count,
            });
        }

        for i in 0..num_outputs {
            let mut info: clap_audio_port_info = unsafe { std::mem::zeroed() };
            let success = unsafe { (audio_ports.get)(&**self.plugin, 0, false, &mut info) };
            if !success {
                anyhow::bail!("Plugin returned an error when querying output port {i} ({num_outputs} total output ports)");
            }

            // TODO: Test whether the channel count matches the port type
            config.inputs.push(AudioPort {
                num_channels: info.channel_count,
            });
        }

        Ok(config)
    }
}
