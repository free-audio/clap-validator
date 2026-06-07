use crate::cli::tracing::{Recordable, Recorder, Span, record};
use crate::plugin::ext::Extension;
use crate::plugin::ext::audio_ports::{AudioPort, check_audio_port_info_valid};
use crate::plugin::instance::Plugin;
use crate::plugin::util::{c_char_slice_to_string, clap_call, cstr_ptr_to_string};
use anyhow::Result;
use clap_sys::ext::audio_ports::clap_audio_port_info;
use clap_sys::ext::audio_ports_config::*;
use clap_sys::id::clap_id;
use std::ffi::{CStr, CString};
use std::mem::zeroed;
use std::ptr::NonNull;

pub struct AudioPortsConfig<'a> {
    plugin: &'a Plugin<'a>,
    audio_ports_config: NonNull<clap_plugin_audio_ports_config>,
}

pub struct AudioPortsConfigInfo<'a> {
    plugin: &'a Plugin<'a>,
    audio_ports_config_info: NonNull<clap_plugin_audio_ports_config_info>,
}

/// A configuration
#[derive(Debug, Clone)]
pub struct AudioPortsConfigConfig {
    pub id: clap_id,
    pub name: String,

    pub input_port_count: u32,
    pub output_port_count: u32,

    pub main_input_port_type: Option<CString>,
    pub main_output_port_type: Option<CString>,

    pub main_input_channel_count: Option<u32>,
    pub main_output_channel_count: Option<u32>,
}

impl<'a> Extension for AudioPortsConfig<'a> {
    const IDS: &'static [&'static CStr] = &[CLAP_EXT_AUDIO_PORTS_CONFIG];

    type Plugin = &'a Plugin<'a>;
    type Struct = clap_plugin_audio_ports_config;

    unsafe fn new(plugin: &'a Plugin<'a>, audio_ports_config: NonNull<Self::Struct>) -> Self {
        Self {
            plugin,
            audio_ports_config,
        }
    }
}

impl<'a> Extension for AudioPortsConfigInfo<'a> {
    const IDS: &'static [&'static CStr] = &[
        CLAP_EXT_AUDIO_PORTS_CONFIG_INFO,
        CLAP_EXT_AUDIO_PORTS_CONFIG_INFO_COMPAT,
    ];

    type Plugin = &'a Plugin<'a>;
    type Struct = clap_plugin_audio_ports_config_info;

    unsafe fn new(plugin: &'a Plugin<'a>, audio_ports_config_info: NonNull<Self::Struct>) -> Self {
        Self {
            plugin,
            audio_ports_config_info,
        }
    }
}

impl AudioPortsConfig<'_> {
    pub fn enumerate(&self) -> Result<Vec<AudioPortsConfigConfig>> {
        (0..self.get_raw_config_count())
            .map(|i| unsafe {
                let info = self.get_raw_config_info(i)?;

                let input_port_type = if info.main_input_port_type.is_null() {
                    None
                } else {
                    Some(CStr::from_ptr(info.main_input_port_type))
                };

                let output_port_type = if info.main_output_port_type.is_null() {
                    None
                } else {
                    Some(CStr::from_ptr(info.main_output_port_type))
                };

                Ok(AudioPortsConfigConfig {
                    id: info.id,
                    name: c_char_slice_to_string(&info.name)?,
                    main_input_port_type: input_port_type.map(|s| s.to_owned()),
                    main_output_port_type: output_port_type.map(|s| s.to_owned()),
                    input_port_count: info.input_port_count,
                    output_port_count: info.output_port_count,
                    main_input_channel_count: info.has_main_input.then_some(info.main_input_channel_count),
                    main_output_channel_count: info.has_main_output.then_some(info.main_output_channel_count),
                })
            })
            .collect()
    }

    pub fn select(&self, config_id: clap_id) -> Result<()> {
        let audio_ports_config = self.audio_ports_config.as_ptr();
        let plugin = self.plugin.as_ptr();

        let span = Span::begin(
            "clap_plugin_audio_ports_config::select",
            record! {
                config_id: config_id
            },
        );

        let result = unsafe {
            clap_call! { audio_ports_config=>select(plugin, config_id) }
        };

        span.finish(record!(result: result));

        if !result {
            anyhow::bail!("audio_ports_config::select() returned false");
        }

        Ok(())
    }

    fn get_raw_config_count(&self) -> u32 {
        let audio_ports_config = self.audio_ports_config.as_ptr();
        let plugin = self.plugin.as_ptr();

        let span = Span::begin("clap_plugin_audio_ports_config::count", ());
        let result = unsafe {
            clap_call! { audio_ports_config=>count(plugin) }
        };

        span.finish(record!(result: result));
        result
    }

    fn get_raw_config_info(&self, index: u32) -> Result<clap_audio_ports_config> {
        let audio_ports_config = self.audio_ports_config.as_ptr();
        let plugin = self.plugin.as_ptr();

        let span = Span::begin(
            "clap_plugin_audio_ports_config::get_raw_config_info",
            record! {
                index: index
            },
        );

        unsafe {
            let mut info = clap_audio_ports_config { ..zeroed() };
            if clap_call! { audio_ports_config=>get(plugin, index, &mut info) } {
                span.finish(record!(result: info));
                Ok(info)
            } else {
                span.finish(record!(result: false));
                anyhow::bail!(
                    "audio_ports_config::get({}) returned false ({} total configs)",
                    index,
                    self.get_raw_config_count()
                );
            }
        }
    }
}

impl AudioPortsConfigInfo<'_> {
    /// Get the current selected audio ports configuration ID.
    pub fn current(&self) -> clap_id {
        let audio_ports_config_info = self.audio_ports_config_info.as_ptr();
        let plugin = self.plugin.as_ptr();

        let span = Span::begin("clap_plugin_audio_ports_config_info::current_config", ());
        let result = unsafe {
            clap_call! { audio_ports_config_info=>current_config(plugin) }
        };

        span.finish(record!(result: result));
        result
    }

    /// Get information about an audio port for a configuration.
    pub fn get(&self, config_id: clap_id, is_input: bool, port_index: u32) -> Result<AudioPort> {
        let info = self.get_raw_port_info(config_id, is_input, port_index)?;
        check_audio_port_info_valid(self.plugin, is_input, port_index, &info)
    }

    fn get_raw_port_info(&self, config_id: clap_id, is_input: bool, port_index: u32) -> Result<clap_audio_port_info> {
        let audio_ports_config_info = self.audio_ports_config_info.as_ptr();
        let plugin = self.plugin.as_ptr();

        let span = Span::begin(
            "clap_plugin_audio_ports_config_info::get",
            record! {
                config_id: config_id,
                is_input: is_input,
                port_index: port_index
            },
        );

        unsafe {
            let mut info = clap_audio_port_info { ..zeroed() };
            if clap_call! { audio_ports_config_info=>get(plugin, config_id, port_index, is_input, &mut info) } {
                span.finish(record!(result: info));
                Ok(info)
            } else {
                span.finish(record!(result: false));
                anyhow::bail!("audio_ports_config_info::get() returned false");
            }
        }
    }
}

impl Recordable for clap_audio_ports_config {
    fn record(&self, record: &mut dyn Recorder) {
        record.record("id", self.id);
        record.record(
            "name",
            c_char_slice_to_string(&self.name).unwrap_or_else(|_| "<invalid utf-8>".to_string()),
        );

        record.record("input_port_count", self.input_port_count);
        record.record("output_port_count", self.output_port_count);

        if self.has_main_input {
            record.record("has_main_input", true);
            record.record("main_input_channel_count", self.main_input_channel_count);

            match unsafe { cstr_ptr_to_string(self.main_input_port_type) } {
                Ok(Some(port_type)) => record.record("main_input_port_type", port_type),
                Ok(None) => record.record("main_input_port_type", "null"),
                Err(_) => record.record("main_input_port_type", "<invalid utf-8>"),
            }
        } else {
            record.record("has_main_input", false);
        }

        if self.has_main_output {
            record.record("has_main_output", true);
            record.record("main_output_channel_count", self.main_output_channel_count);

            match unsafe { cstr_ptr_to_string(self.main_output_port_type) } {
                Ok(Some(port_type)) => record.record("main_output_port_type", port_type),
                Ok(None) => record.record("main_output_port_type", "null"),
                Err(_) => record.record("main_output_port_type", "<invalid utf-8>"),
            }
        } else {
            record.record("has_main_output", false);
        }
    }
}
