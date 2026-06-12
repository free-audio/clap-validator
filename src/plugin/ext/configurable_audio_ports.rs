use crate::cli::tracing::{Recordable, Recorder, Span, from_fn, record};
use crate::plugin::ext::Extension;
use crate::plugin::instance::Plugin;
use crate::plugin::util::clap_call;
use clap_sys::ext::ambisonic::{CLAP_PORT_AMBISONIC, clap_ambisonic_config};
use clap_sys::ext::audio_ports::{CLAP_PORT_MONO, CLAP_PORT_STEREO};
use clap_sys::ext::configurable_audio_ports::*;
use clap_sys::ext::surround::*;
use std::ffi::CStr;
use std::fmt::{Debug, Display};
use std::ptr::{NonNull, null};

#[derive(Debug, Clone, Copy)]
pub struct AudioPortsRequest<'a> {
    pub is_input: bool,
    pub port_index: u32,
    pub request_info: AudioPortsRequestInfo<'a>,
}

/// Different types of port details that can be requested.
#[derive(Debug, Clone, Copy)]
pub enum AudioPortsRequestInfo<'a> {
    Mono,
    Stereo,
    Untyped {
        channel_count: u32,
    },

    Ambisonic {
        channel_count: u32,
        config: &'a clap_ambisonic_config,
    },

    Surround {
        channel_map: &'a [u8],
    },
}

pub struct ConfigurableAudioPorts<'a> {
    plugin: &'a Plugin<'a>,
    configurable_audio_ports: NonNull<clap_plugin_configurable_audio_ports>,
}

impl<'a> Extension for ConfigurableAudioPorts<'a> {
    const IDS: &'static [&'static CStr] = &[
        CLAP_EXT_CONFIGURABLE_AUDIO_PORTS,
        CLAP_EXT_CONFIGURABLE_AUDIO_PORTS_COMPAT,
    ];

    type Plugin = &'a Plugin<'a>;
    type Struct = clap_plugin_configurable_audio_ports;

    unsafe fn new(plugin: &'a Plugin<'a>, configurable_audio_ports: NonNull<Self::Struct>) -> Self {
        Self {
            plugin,
            configurable_audio_ports,
        }
    }
}

impl<'a> ConfigurableAudioPorts<'a> {
    pub fn can_apply_configuration(&self, requests: &[AudioPortsRequest]) -> bool {
        self.plugin.status().assert_inactive();

        let span = Span::begin(
            "clap_plugin_configurable_audio_ports::can_apply_configuration",
            from_fn(|record| {
                for (i, request) in requests.iter().enumerate() {
                    record.record(&format!("requests.{}", i), *request);
                }
            }),
        );

        let requests = convert_requests(requests.iter().copied());
        let plugin = self.plugin.as_ptr();
        let ext = self.configurable_audio_ports.as_ptr();

        unsafe {
            let result = clap_call! { ext=>can_apply_configuration(
                plugin,
                requests.as_ptr(),
                requests.len() as u32
            )};

            span.finish(record!(result: result));
            result
        }
    }

    pub fn apply_configuration(&self, requests: &[AudioPortsRequest]) -> bool {
        self.plugin.status().assert_inactive();

        let span = Span::begin(
            "clap_plugin_configurable_audio_ports::apply_configuration",
            from_fn(|record| {
                for (i, request) in requests.iter().enumerate() {
                    record.record(&format!("requests.{}", i), *request);
                }
            }),
        );

        let requests = convert_requests(requests.iter().copied());
        let plugin = self.plugin.as_ptr();
        let ext = self.configurable_audio_ports.as_ptr();

        unsafe {
            let result = clap_call! { ext=>apply_configuration(
                plugin,
                requests.as_ptr(),
                requests.len() as u32
            )};

            span.finish(record!(result: result));
            result
        }
    }
}

impl<'a> AudioPortsRequestInfo<'a> {
    pub fn channel_count(&self) -> u32 {
        match self {
            AudioPortsRequestInfo::Mono => 1,
            AudioPortsRequestInfo::Stereo => 2,
            AudioPortsRequestInfo::Untyped { channel_count } => *channel_count,
            AudioPortsRequestInfo::Ambisonic { channel_count, .. } => *channel_count,
            AudioPortsRequestInfo::Surround { channel_map } => channel_map.len() as u32,
        }
    }

    pub fn port_type(&self) -> Option<&'a CStr> {
        match self {
            AudioPortsRequestInfo::Mono => Some(CLAP_PORT_MONO),
            AudioPortsRequestInfo::Stereo => Some(CLAP_PORT_STEREO),
            AudioPortsRequestInfo::Ambisonic { .. } => Some(CLAP_PORT_AMBISONIC),
            AudioPortsRequestInfo::Surround { .. } => Some(CLAP_PORT_SURROUND),
            AudioPortsRequestInfo::Untyped { .. } => None,
        }
    }
}

fn convert_requests<'a>(
    requests: impl IntoIterator<Item = AudioPortsRequest<'a>>,
) -> Vec<clap_audio_port_configuration_request> {
    requests
        .into_iter()
        .map(|r| clap_audio_port_configuration_request {
            is_input: r.is_input,
            port_index: r.port_index,
            channel_count: r.request_info.channel_count(),
            port_type: r.request_info.port_type().map_or(null(), |f| f.as_ptr()),
            port_details: match r.request_info {
                AudioPortsRequestInfo::Surround { channel_map } => channel_map.as_ptr() as *const _,
                AudioPortsRequestInfo::Ambisonic { config, .. } => config as *const clap_ambisonic_config as *const _,
                _ => null(),
            },
        })
        .collect::<Vec<_>>()
}

impl Recordable for AudioPortsRequest<'_> {
    fn record(&self, record: &mut dyn Recorder) {
        record.record("is_input", self.is_input);
        record.record("port_index", self.port_index);
        record.record("details", self.request_info);
    }
}

impl Recordable for AudioPortsRequestInfo<'_> {
    fn record(&self, record: &mut dyn Recorder) {
        fn surround_map_to_string(channel_map: &[u8]) -> String {
            channel_map
                .iter()
                .map(|&ch| match ch as u32 {
                    CLAP_SURROUND_FL => "FL",
                    CLAP_SURROUND_FR => "FR",
                    CLAP_SURROUND_FC => "FC",
                    CLAP_SURROUND_LFE => "LFE",
                    CLAP_SURROUND_BL => "BL",
                    CLAP_SURROUND_BR => "BR",
                    CLAP_SURROUND_FLC => "FLC",
                    CLAP_SURROUND_FRC => "FRC",
                    CLAP_SURROUND_BC => "BC",
                    CLAP_SURROUND_SL => "SL",
                    CLAP_SURROUND_SR => "SR",
                    CLAP_SURROUND_TC => "TC",
                    CLAP_SURROUND_TFL => "TFL",
                    CLAP_SURROUND_TFC => "TFC",
                    CLAP_SURROUND_TFR => "TFR",
                    CLAP_SURROUND_TBL => "TBL",
                    CLAP_SURROUND_TBC => "TBC",
                    CLAP_SURROUND_TBR => "TBR",
                    _ => "?",
                })
                .collect::<Vec<_>>()
                .join(" ")
        }

        match self {
            AudioPortsRequestInfo::Mono => {
                record.record("type", "mono");
                record.record("channel_count", 1);
            }
            AudioPortsRequestInfo::Stereo => {
                record.record("type", "stereo");
                record.record("channel_count", 2);
            }
            AudioPortsRequestInfo::Untyped { channel_count } => {
                record.record("type", "null");
                record.record("channel_count", *channel_count);
            }
            AudioPortsRequestInfo::Ambisonic { channel_count, config } => {
                record.record("type", "ambisonic");
                record.record("channel_count", *channel_count);
                record.record("config", config);
            }
            AudioPortsRequestInfo::Surround { channel_map } => {
                record.record("type", "surround");
                record.record("channel_count", channel_map.len() as u32);
                record.record("channel_map", surround_map_to_string(channel_map));
            }
        }
    }
}

impl Display for AudioPortsRequest<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} #{}: {}",
            if self.is_input { "Input" } else { "Output" },
            self.port_index,
            self.request_info
        )
    }
}

impl Display for AudioPortsRequestInfo<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AudioPortsRequestInfo::Mono => write!(f, "Mono"),
            AudioPortsRequestInfo::Stereo => write!(f, "Stereo"),
            AudioPortsRequestInfo::Untyped { channel_count } => {
                write!(f, "Untyped ({}ch)", channel_count)
            }
            AudioPortsRequestInfo::Ambisonic { channel_count, .. } => {
                write!(f, "Ambisonic ({}ch)", channel_count)
            }
            AudioPortsRequestInfo::Surround { channel_map } => {
                write!(f, "Surround ({}ch)", channel_map.len())
            }
        }
    }
}
