//! Abstractions for interacting with the `audio-ports` extension.

use anyhow::Result;
use clap_sys::ext::audio_ports::{
    clap_audio_port_info, clap_plugin_audio_ports, CLAP_EXT_AUDIO_PORTS,
};
use clap_sys::id::CLAP_INVALID_ID;
use std::os::raw::c_char;
use std::ptr::NonNull;

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
    /// The index if the output/input port this input/output port should be connected to.
    pub in_place_pair_idx: Option<usize>,
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
        let num_inputs = unsafe { (audio_ports.count)(self.plugin.as_ptr(), true) };
        let num_outputs = unsafe { (audio_ports.count)(self.plugin.as_ptr(), false) };

        for i in 0..num_inputs {
            let mut info: clap_audio_port_info = unsafe { std::mem::zeroed() };
            let success = unsafe { (audio_ports.get)(self.plugin.as_ptr(), 0, true, &mut info) };
            if !success {
                anyhow::bail!("Plugin returned an error when querying input audio port {i} ({num_inputs} total input ports)");
            }

            let in_place_pair_idx = if info.in_place_pair == CLAP_INVALID_ID {
                None
            } else {
                Some(info.in_place_pair as usize)
            };
            if let Some(in_place_pair_idx) = in_place_pair_idx {
                if in_place_pair_idx >= num_outputs as usize {
                    anyhow::bail!("Input port {i} has an in-place pair index for output port {in_place_pair_idx}, but there are only {num_outputs} output ports");
                }
            }

            // TODO: Test whether the channel count matches the port type
            config.inputs.push(AudioPort {
                num_channels: info.channel_count,
                in_place_pair_idx,
            });
        }

        for i in 0..num_outputs {
            let mut info: clap_audio_port_info = unsafe { std::mem::zeroed() };
            let success = unsafe { (audio_ports.get)(self.plugin.as_ptr(), 0, false, &mut info) };
            if !success {
                anyhow::bail!("Plugin returned an error when querying output audio port {i} ({num_outputs} total output ports)");
            }

            let in_place_pair_idx = if info.in_place_pair == CLAP_INVALID_ID {
                None
            } else {
                Some(info.in_place_pair as usize)
            };
            if let Some(in_place_pair_idx) = in_place_pair_idx {
                if in_place_pair_idx >= num_outputs as usize {
                    anyhow::bail!("Input port {i} has an in-place pair index for input port {in_place_pair_idx}, but there are only {num_inputs} input ports");
                }
            }

            // TODO: Test whether the channel count matches the port type
            config.outputs.push(AudioPort {
                num_channels: info.channel_count,
                in_place_pair_idx,
            });
        }

        Ok(config)
    }
}
