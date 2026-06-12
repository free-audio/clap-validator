//! Abstractions for interacting with the `audio-ports` extension.

use super::Extension;
use crate::cli::tracing::{Recordable, Recorder, Span, record};
use crate::plugin::ext::ambisonic::Ambisonic;
use crate::plugin::ext::surround::Surround;
use crate::plugin::instance::Plugin;
use crate::plugin::util::{clap_call, cstr_ptr_to_string};
use anyhow::{Context, Result};
use clap_sys::ext::ambisonic::CLAP_PORT_AMBISONIC;
use clap_sys::ext::audio_ports::*;
use clap_sys::ext::surround::CLAP_PORT_SURROUND;
use clap_sys::id::{CLAP_INVALID_ID, clap_id};
use std::collections::HashSet;
use std::ffi::{CStr, CString};
use std::mem::zeroed;
use std::ptr::NonNull;

/// Abstraction for the `audio-ports` extension covering the main thread functionality.
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioPort {
    /// Stable ID of the audio port.
    pub id: clap_id,

    /// Whether this is the main audio port.
    pub is_main: bool,

    pub port_type: Option<CString>,

    /// The number of channels for an audio port.
    pub channel_count: u32,

    /// The stable ID of the output/input port this input/output port should be connected to.
    pub in_place_pair: Option<clap_id>,

    /// Supports 64 bit processing
    pub supports_double_sample_size: bool,

    /// Prefers 64 bit processing
    pub prefers_double_sample_size: bool,

    /// All ports with this flag require common sample size
    pub requires_common_sample_size: bool,
}

impl<'a> Extension for AudioPorts<'a> {
    const IDS: &'static [&'static CStr] = &[CLAP_EXT_AUDIO_PORTS];

    type Plugin = &'a Plugin<'a>;
    type Struct = clap_plugin_audio_ports;

    unsafe fn new(plugin: &'a Plugin<'a>, audio_ports: NonNull<Self::Struct>) -> Self {
        Self { plugin, audio_ports }
    }
}

impl AudioPorts<'_> {
    /// Get the audio port configuration for this plugin. This automatically performs a number of
    /// consistency checks on the plugin's audio port configuration.
    pub fn config(&self) -> Result<AudioPortConfig> {
        let mut config = AudioPortConfig::default();
        let num_inputs = self.get_raw_port_count(true);
        let num_outputs = self.get_raw_port_count(false);

        for index in 0..num_inputs {
            let info = match self.get_raw_port_info(true, index) {
                Some(info) => info,
                None => {
                    anyhow::bail!(
                        "Plugin returned false when querying audio port info for input port {index} (out of \
                         {num_inputs} total)"
                    );
                }
            };

            config.inputs.push(
                check_audio_port_info_valid(self.plugin, true, index, &info)
                    .with_context(|| format!("Inconsistent port info for input audio port {index}"))?,
            );
        }

        for index in 0..num_outputs {
            let info = match self.get_raw_port_info(false, index) {
                Some(info) => info,
                None => {
                    anyhow::bail!(
                        "Plugin returned false when querying audio port info for output port {index} (out of \
                         {num_outputs} total)"
                    );
                }
            };

            config.outputs.push(
                check_audio_port_info_valid(self.plugin, false, index, &info)
                    .with_context(|| format!("Inconsistent port info for output audio port {index}"))?,
            );
        }

        let has_single_precision_requires_common_port = config
            .inputs
            .iter()
            .chain(config.outputs.iter())
            .any(|port| port.requires_common_sample_size && !port.supports_double_sample_size);

        let has_double_precision_requires_common_port = config
            .inputs
            .iter()
            .chain(config.outputs.iter())
            .any(|port| port.requires_common_sample_size && port.supports_double_sample_size);

        // this implies that the common sample size requirement is useless (i.e. every port can only support
        // 32bit sample size) and nullifies the 64 bit support of the other ports
        if has_single_precision_requires_common_port && has_double_precision_requires_common_port {
            anyhow::bail!(
                "The plugin has audio ports that require common sample size, but some of these ports only support \
                 32-bit sample size while others support 64-bit sample size."
            );
        }

        // check for duplicate stable IDs
        for is_input in [true, false] {
            let mut ids = HashSet::new();
            let ports = if is_input { &config.inputs } else { &config.outputs };

            for (index, port) in ports.iter().enumerate() {
                if !ids.insert(port.id) {
                    anyhow::bail!(
                        "Found {} audio port ({}) with a duplicate ID ({}).",
                        if is_input { "input" } else { "output" },
                        index,
                        port.id
                    );
                }
            }
        }

        Ok(config)
    }

    fn get_raw_port_count(&self, is_input: bool) -> u32 {
        let audio_ports = self.audio_ports.as_ptr();
        let plugin = self.plugin.as_ptr();

        let span = Span::begin(
            "clap_plugin_audio_ports::count",
            record! {
                is_input: is_input
            },
        );

        let result = unsafe {
            clap_call! { audio_ports=>count(plugin, is_input) }
        };

        span.finish(record!(result: result));
        result
    }

    fn get_raw_port_info(&self, is_input: bool, port_index: u32) -> Option<clap_audio_port_info> {
        let audio_ports = self.audio_ports.as_ptr();
        let plugin = self.plugin.as_ptr();

        let span = Span::begin(
            "clap_plugin_audio_ports::get",
            record! {
                is_input: is_input,
                port_index: port_index
            },
        );

        unsafe {
            let mut info = clap_audio_port_info { ..zeroed() };
            if clap_call! { audio_ports=>get(plugin, port_index, is_input, &mut info) } {
                span.finish(record!(result: info));
                Some(info)
            } else {
                None
            }
        }
    }
}

pub fn check_audio_port_info_valid(
    plugin: &Plugin,
    is_input: bool,
    port_index: u32,
    info: &clap_audio_port_info,
) -> Result<AudioPort> {
    let ext_ambisonic = plugin.get_extension::<Ambisonic>();
    let ext_surround = plugin.get_extension::<Surround>();

    if info.id == CLAP_INVALID_ID {
        anyhow::bail!("The stable ID is `CLAP_INVALID_ID`.");
    }

    // if the main port flag is set, the port index must be 0
    let is_main = (info.flags & CLAP_AUDIO_PORT_IS_MAIN) != 0;
    if is_main && port_index != 0 {
        anyhow::bail!("Port is marked as main, but it is not the first port in the list.");
    }

    let supports_double_sample_size = (info.flags & CLAP_AUDIO_PORT_SUPPORTS_64BITS) != 0;
    let requires_common_sample_size = (info.flags & CLAP_AUDIO_PORT_REQUIRES_COMMON_SAMPLE_SIZE) != 0;
    let prefers_double_sample_size = (info.flags & CLAP_AUDIO_PORT_PREFERS_64BITS) != 0;

    if !supports_double_sample_size && prefers_double_sample_size {
        anyhow::bail!("Port prefers 64-bit sample size, but does not support it.");
    }

    let port_type = if info.port_type.is_null() {
        None
    } else {
        Some(unsafe { CStr::from_ptr(info.port_type) })
    };

    // check consistency between port type and channel count / extensions
    check_audio_port_type_consistent(
        is_input,
        port_index,
        port_type,
        info.channel_count,
        ext_ambisonic.as_ref(),
        ext_surround.as_ref(),
    )?;

    Ok(AudioPort {
        id: info.id,
        is_main: (info.flags & CLAP_AUDIO_PORT_IS_MAIN) != 0,
        channel_count: info.channel_count,
        port_type: port_type.map(|s| s.to_owned()),
        in_place_pair: if info.in_place_pair == CLAP_INVALID_ID {
            None
        } else {
            Some(info.in_place_pair)
        },

        supports_double_sample_size,
        requires_common_sample_size,
        prefers_double_sample_size,
    })
}

/// Check if the returned port information consistent with the audio port type, ambisonic extension, surround extension, etc.
/// Returns an error if the port information is not consistent.
pub fn check_audio_port_type_consistent(
    is_input: bool,
    port_index: u32,
    port_type: Option<&CStr>,
    channel_count: u32,
    ext_ambisonic: Option<&Ambisonic>,
    ext_surround: Option<&Surround>,
) -> Result<()> {
    if port_type.is_none() {
        return Ok(());
    }

    if port_type == Some(CLAP_PORT_MONO) {
        if channel_count == 1 {
            Ok(())
        } else {
            anyhow::bail!(
                "Audio port type is 'mono', but the audio port has {} channels.",
                channel_count
            );
        }
    } else if port_type == Some(CLAP_PORT_STEREO) {
        if channel_count == 2 {
            Ok(())
        } else {
            anyhow::bail!(
                "Audio port type is 'stereo', but the audio port has {} channel(s).",
                channel_count
            );
        }
    } else if port_type == Some(CLAP_PORT_SURROUND) {
        let Some(ext_surround) = ext_surround else {
            anyhow::bail!("Audio port type is 'surround', but the plugin does not implement the 'surround' extension.");
        };

        let channel_map = ext_surround.get_channel_map(is_input, port_index, channel_count);
        if channel_map.len() as u32 != channel_count {
            anyhow::bail!(
                "The surround channel map returned by 'clap_plugin_surround::get_channel_map' has length {}, but the \
                 audio port has {} channels.",
                channel_map.len(),
                channel_count
            );
        }

        let mask = channel_map.iter().fold(0u64, |acc, &ch| acc | (1u64 << ch));
        if !ext_surround.is_channel_mask_supported(mask) {
            anyhow::bail!(
                "The surround channel mask {mask:#b} returned by 'clap_plugin_surround::get_channel_map' is not \
                 supported by the plugin ('clap_plugin_surround::is_channel_mask_supported' returned false)."
            );
        }

        Ok(())
    } else if port_type == Some(CLAP_PORT_AMBISONIC) {
        let Some(ext_ambisonic) = ext_ambisonic else {
            anyhow::bail!(
                "Audio port type is 'ambisonic', but the plugin does not implement the 'ambisonic' extension."
            );
        };

        // ambisonic audio requires (N^2) channels where N is the ambisonics order
        if channel_count.isqrt().pow(2) != channel_count {
            anyhow::bail!(
                "Expected a perfect square (N^2 where N is the ambisonics order) number of channels for ambisonic \
                 audio port, but the audio port has {} channels.",
                channel_count
            );
        }

        let config = ext_ambisonic
            .get_config(is_input, port_index)
            .context("Failed to get ambisonic configuration for the port.")?;

        if !ext_ambisonic.is_config_supported(&config) {
            anyhow::bail!(
                "The ambisonic configuration returned by 'clap_plugin_ambisonic::get_config' is not supported by the \
                 plugin ('clap_plugin_ambisonic::is_config_supported' returned false).",
            );
        }

        Ok(())
    } else {
        log::warn!("Unknown audio port type '{port_type:?}'");
        Ok(())
    }
}

impl Recordable for clap_audio_port_info {
    fn record(&self, record: &mut dyn Recorder) {
        record.record("id", self.id);
        record.record("channel_count", self.channel_count);

        record.record("flags.is_main", self.flags & CLAP_AUDIO_PORT_IS_MAIN != 0);
        record.record(
            "flags.supports_double_sample_size",
            self.flags & CLAP_AUDIO_PORT_SUPPORTS_64BITS != 0,
        );
        record.record(
            "flags.prefers_double_sample_size",
            self.flags & CLAP_AUDIO_PORT_PREFERS_64BITS != 0,
        );
        record.record(
            "flags.requires_common_sample_size",
            self.flags & CLAP_AUDIO_PORT_REQUIRES_COMMON_SAMPLE_SIZE != 0,
        );

        match unsafe { cstr_ptr_to_string(self.port_type) } {
            Ok(Some(port_type)) => record.record("port_type", port_type),
            Ok(None) => record.record("port_type", "null"),
            Err(_) => record.record("port_type", "<invalid utf-8>"),
        }

        if self.in_place_pair == CLAP_INVALID_ID {
            record.record("in_place_pair", "<invalid>");
        } else {
            record.record("in_place_pair", self.in_place_pair);
        }
    }
}
