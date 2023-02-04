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
use std::collections::HashMap;
use std::ffi::CStr;
use std::ptr::NonNull;

use crate::plugin::instance::Plugin;
use crate::util::unsafe_clap_call;

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
    /// The index if the output/input port this input/output port should be connected to. This is
    /// the index in the other **port list**, not a stable ID (which have already been translated).
    pub in_place_pair_idx: Option<usize>,
}

impl<'a> Extension<&'a Plugin<'a>> for AudioPorts<'a> {
    const EXTENSION_ID: &'static CStr = CLAP_EXT_AUDIO_PORTS;

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

        // TODO: Refactor this to reduce the duplication a little without hurting the human readable error messages
        let audio_ports = self.audio_ports.as_ptr();
        let plugin = self.plugin.as_ptr();
        let num_inputs = unsafe_clap_call! { audio_ports=>count(plugin, true) };
        let num_outputs = unsafe_clap_call! { audio_ports=>count(plugin, false) };

        // Audio ports have a stable ID attribute that can be used to connect input and output ports
        // so the host can do in-place processing. This uses stable IDs rather than the indices in
        // the list. To make it easier for us, we'll translate those stable IDs to vector indices.
        // These two hashmaps are keyed by the port's stable ID, and the value is a pair containing
        // the port's index in the input/output port vector, and the stable ID of its in-place pair
        // port.
        let mut input_stable_index_pairs: HashMap<u32, (usize, u32)> = HashMap::new();
        let mut output_stable_index_pairs: HashMap<u32, (usize, u32)> = HashMap::new();

        for i in 0..num_inputs {
            let mut info: clap_audio_port_info = unsafe { std::mem::zeroed() };
            let success = unsafe_clap_call! { audio_ports=>get(plugin, i, true, &mut info) };
            if !success {
                anyhow::bail!(
                    "Plugin returned an error when querying input audio port {i} ({num_inputs} \
                     total input ports)."
                );
            }

            is_audio_port_type_consistent(&info).with_context(|| {
                format!(
                    "Inconsistent channel count for output port {i} ({num_outputs} total output \
                     ports)"
                )
            })?;

            // We'll convert these stable IDs to vector indices later
            if input_stable_index_pairs.contains_key(&info.id) {
                anyhow::bail!(
                    "The stable ID of input audio port {i} ({}) is a duplicate.",
                    info.id
                );
            }
            input_stable_index_pairs.insert(info.id, (i as usize, info.in_place_pair));

            config.inputs.push(AudioPort {
                num_channels: info.channel_count,
                // These are reconstructed from `input_stable_index_pairs` and
                // `output_stable_index_pairs` later
                in_place_pair_idx: None,
            });
        }

        for i in 0..num_outputs {
            let mut info: clap_audio_port_info = unsafe { std::mem::zeroed() };
            let success = unsafe_clap_call! { audio_ports=>get(plugin, i, false, &mut info) };
            if !success {
                anyhow::bail!(
                    "Plugin returned an error when querying output audio port {i} ({num_outputs} \
                     total output ports)."
                );
            }

            is_audio_port_type_consistent(&info).with_context(|| {
                format!(
                    "Inconsistent channel count for output port {i} ({num_outputs} total output \
                     ports)"
                )
            })?;

            if output_stable_index_pairs.contains_key(&info.id) {
                anyhow::bail!(
                    "The stable ID of output audio port {i} ({}) is a duplicate.",
                    info.id
                );
            }
            output_stable_index_pairs.insert(info.id, (i as usize, info.in_place_pair));

            config.outputs.push(AudioPort {
                num_channels: info.channel_count,
                in_place_pair_idx: None,
            });
        }

        // Now we need to convert the stable in-place pair indices to vector indices
        for (input_stable_id, (input_port_idx, pair_stable_id)) in input_stable_index_pairs
            .iter()
            .filter(|(_, (_, pair_stable_id))| *pair_stable_id != CLAP_INVALID_ID)
        {
            match output_stable_index_pairs
                .iter()
                .find(|(output_stable_id, (_, _))| *output_stable_id == pair_stable_id)
            {
                // This relation should be symmetrical
                Some((_, (pair_output_port_idx, output_pair_stable_id)))
                    if output_pair_stable_id == input_stable_id =>
                {
                    config.inputs[*input_port_idx].in_place_pair_idx = Some(*pair_output_port_idx);
                    config.inputs[*pair_output_port_idx].in_place_pair_idx = Some(*input_port_idx);
                }
                Some((output_stable_id, (pair_output_port_idx, output_pair_stable_id))) => {
                    anyhow::bail!(
                        "Input port {input_port_idx} with stable ID {input_stable_id} is \
                         connected to output port {pair_output_port_idx} with stable ID \
                         {output_stable_id} through an in-place pair, but the relation is not \
                         symmetrical. The output port reports to have an in-place pair with \
                         stable ID {output_pair_stable_id}."
                    )
                }
                None => anyhow::bail!(
                    "Input port {input_port_idx} with stable ID {input_stable_id} claims to be \
                     connected to an output port with stable ID {pair_stable_id} through an \
                     in-place pair, but this port does not exist."
                ),
            }
        }

        // This needs to be repeated for output ports that are connected to input ports in case an
        // output port has a stable ID pair but the corresponding input port does not
        for (output_stable_id, (output_port_idx, pair_stable_id)) in output_stable_index_pairs
            .iter()
            .filter(|(_, (_, pair_stable_id))| *pair_stable_id != CLAP_INVALID_ID)
        {
            match input_stable_index_pairs
                .iter()
                .find(|(input_stable_id, (_, _))| *input_stable_id == pair_stable_id)
            {
                Some((_, (pair_input_port_idx, input_pair_stable_id)))
                    if input_pair_stable_id == output_stable_id =>
                {
                    // We should have already done this. If this is not the case, then this is an
                    // error in the validator
                    assert_eq!(
                        config.inputs[*output_port_idx].in_place_pair_idx,
                        Some(*pair_input_port_idx)
                    );
                    assert_eq!(
                        config.inputs[*pair_input_port_idx].in_place_pair_idx,
                        Some(*output_port_idx)
                    );
                }
                Some((input_stable_id, (pair_input_port_idx, input_pair_stable_id))) => {
                    anyhow::bail!(
                        "Output port {output_port_idx} with stable ID {output_stable_id} is \
                         connected to input port {pair_input_port_idx} with stable ID \
                         {input_stable_id} through an in-place pair, but the relation is not \
                         symmetrical. The input port reports to have an in-place pair with stable \
                         ID {input_pair_stable_id}."
                    )
                }
                None => anyhow::bail!(
                    "Output port {output_port_idx} with stable ID {output_stable_id} claims to be \
                     connected to an input port with stable ID {pair_stable_id} through an \
                     in-place pair, but this port does not exist."
                ),
            }
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
    if port_type == CLAP_PORT_MONO {
        if info.channel_count == 1 {
            Ok(())
        } else {
            anyhow::bail!(
                "Expected 1 channel, but the audio port has {} channels.",
                info.channel_count
            );
        }
    } else if port_type == CLAP_PORT_STEREO {
        if info.channel_count == 2 {
            Ok(())
        } else {
            anyhow::bail!(
                "Expected 2 channels, but the audio port has {} channel(s).",
                info.channel_count
            );
        }
    } else if port_type == CLAP_PORT_SURROUND
        || port_type == CLAP_PORT_CV
        || port_type == CLAP_PORT_AMBISONIC
    {
        // TODO: Test the channel counts by querying those extensions
        Ok(())
    } else {
        log::debug!("TODO: Unknown audio port type '{port_type:?}'");
        Ok(())
    }
}

impl AudioPortConfig {
    /// Create a pair of zero initialized `(input_buffers, output_buffers)` for this audio port
    /// configuration. These can be bassed with
    /// [`ProcessData`][super::audio_thread::process::ProcessData] to create a process data struct.
    #[allow(clippy::type_complexity)]
    pub fn create_buffers(&self, buffer_size: usize) -> (Vec<Vec<Vec<f32>>>, Vec<Vec<Vec<f32>>>) {
        let input_buffers: Vec<Vec<Vec<f32>>> = self
            .inputs
            .iter()
            .map(|port_config| vec![vec![0.0; buffer_size]; port_config.num_channels as usize])
            .collect();
        let output_buffers: Vec<Vec<Vec<f32>>> = self
            .outputs
            .iter()
            .map(|port_config| vec![vec![0.0; buffer_size]; port_config.num_channels as usize])
            .collect();

        (input_buffers, output_buffers)
    }
}
