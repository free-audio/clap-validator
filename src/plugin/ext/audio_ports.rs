//! Abstractions for interacting with the `audio-ports` extension.

use anyhow::{Context, Result};
use clap_sys::ext::audio_ports::{
    clap_audio_port_info, clap_plugin_audio_ports, CLAP_EXT_AUDIO_PORTS, CLAP_PORT_MONO,
    CLAP_PORT_STEREO,
};
use clap_sys::ext::draft::ambisonic::CLAP_PORT_AMBISONIC;
use clap_sys::ext::draft::cv::CLAP_PORT_CV;
use clap_sys::ext::draft::surround::CLAP_PORT_SURROUND;
use clap_sys::id::CLAP_INVALID_ID;
use std::ffi::CStr;
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
    /// Get the audio port configuration for this plugin. This automatically performs a number of
    /// consistency checks on the plugin's audio port configuration.
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

            is_audio_port_type_consistent(&info).with_context(|| format!("Inconsistent channel count for output port {i} ({num_outputs} total output ports)"))?;

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
                    anyhow::bail!("Output port {i} has an in-place pair index for input port {in_place_pair_idx}, but there are only {num_inputs} input ports");
                }
            }

            is_audio_port_type_consistent(&info).with_context(|| format!("Inconsistent channel count for output port {i} ({num_outputs} total output ports)"))?;

            config.outputs.push(AudioPort {
                num_channels: info.channel_count,
                in_place_pair_idx,
            });
        }

        Ok(config)
    }
}

/// Check whether the number of channels matches an audio port's type string, if that is set.
/// Returns an error if the port type is not consistent
fn is_audio_port_type_consistent(info: &clap_audio_port_info) -> Result<()> {
    if info.port_type.is_null() {
        return Ok(());
    }

    let port_type = unsafe { CStr::from_ptr(info.port_type) };
    if port_type == unsafe { CStr::from_ptr(CLAP_PORT_MONO) } {
        if info.channel_count == 1 {
            Ok(())
        } else {
            anyhow::bail!(
                "Expected 1 channel, but the audio port has {} channels",
                info.channel_count
            );
        }
    } else if port_type == unsafe { CStr::from_ptr(CLAP_PORT_STEREO) } {
        if info.channel_count == 2 {
            Ok(())
        } else {
            anyhow::bail!(
                "Expected 2 channels, but the audio port has {} channel(s)",
                info.channel_count
            );
        }
    } else if port_type == unsafe { CStr::from_ptr(CLAP_PORT_SURROUND) }
        || port_type == unsafe { CStr::from_ptr(CLAP_PORT_CV) }
        || port_type == unsafe { CStr::from_ptr(CLAP_PORT_AMBISONIC) }
    {
        // TODO: Test the channel counts by querying those extensions
        Ok(())
    } else {
        eprintln!("TODO: Unknown audio port type '{port_type:?}'");
        Ok(())
    }
}
