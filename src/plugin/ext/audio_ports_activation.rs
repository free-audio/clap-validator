use crate::cli::tracing::{Span, record};
use crate::plugin::ext::Extension;
use crate::plugin::instance::{Plugin, PluginAudioThread};
use crate::plugin::util::clap_call;
use clap_sys::ext::audio_ports_activation::*;
use std::ffi::CStr;
use std::ptr::NonNull;

/// Abstraction for the `audio-ports-activation` extension covering the main thread functionality.
pub struct AudioPortsActivation<'a> {
    plugin: &'a Plugin<'a>,
    audio_ports_activation: NonNull<clap_plugin_audio_ports_activation>,
}

pub struct AudioPortsActivationAudio<'a> {
    plugin: &'a PluginAudioThread<'a>,
    audio_ports_activation: NonNull<clap_plugin_audio_ports_activation>,
}

impl<'a> Extension for AudioPortsActivation<'a> {
    const IDS: &'static [&'static CStr] = &[CLAP_EXT_AUDIO_PORTS_ACTIVATION, CLAP_EXT_AUDIO_PORTS_ACTIVATION_COMPAT];

    type Plugin = &'a Plugin<'a>;
    type Struct = clap_plugin_audio_ports_activation;

    unsafe fn new(plugin: &'a Plugin<'a>, extension_struct: NonNull<Self::Struct>) -> Self {
        Self {
            plugin,
            audio_ports_activation: extension_struct,
        }
    }
}

impl<'a> Extension for AudioPortsActivationAudio<'a> {
    const IDS: &'static [&'static CStr] = &[CLAP_EXT_AUDIO_PORTS_ACTIVATION, CLAP_EXT_AUDIO_PORTS_ACTIVATION_COMPAT];

    type Plugin = &'a PluginAudioThread<'a>;
    type Struct = clap_plugin_audio_ports_activation;

    unsafe fn new(plugin: &'a PluginAudioThread<'a>, audio_ports_activation: NonNull<Self::Struct>) -> Self {
        Self {
            plugin,
            audio_ports_activation,
        }
    }
}

impl<'a> AudioPortsActivation<'a> {
    pub fn can_activate_while_processing(&self) -> bool {
        let audio_ports_activation = self.audio_ports_activation.as_ptr();
        let plugin = self.plugin.as_ptr();

        let span = Span::begin("clap_plugin_audio_ports_activation::can_activate_while_processing", ());
        let result = unsafe {
            clap_call! { audio_ports_activation=>can_activate_while_processing(plugin) }
        };

        span.finish(record!(result: result));
        result
    }

    /// Activates or deactivates a single audio port.
    pub fn set_active(&self, is_input: bool, port_index: u32, is_active: bool, sample_size: u32) -> bool {
        self.plugin.status().assert_inactive();

        let audio_ports_activation = self.audio_ports_activation.as_ptr();
        let plugin = self.plugin.as_ptr();

        let span = Span::begin(
            "clap_plugin_audio_ports_activation::set_active",
            record! {
                is_input: is_input,
                port_index: port_index,
                is_active: is_active,
                sample_size: sample_size
            },
        );

        let result = unsafe {
            clap_call! { audio_ports_activation=>set_active(plugin, is_input, port_index, is_active, sample_size) }
        };

        span.finish(record!(result: result));
        result
    }
}

impl<'a> AudioPortsActivationAudio<'a> {
    /// Activates or deactivates a single audio port. Only allowed if `can_activate_while_processing` returns `true`.
    pub fn set_active(&self, is_input: bool, port_index: u32, is_active: bool, sample_size: u32) -> bool {
        self.plugin.status().assert_active();

        let audio_ports_activation = self.audio_ports_activation.as_ptr();
        let plugin = self.plugin.as_ptr();

        let span = Span::begin(
            "clap_plugin_audio_ports_activation::set_active",
            record! {
                is_input: is_input,
                port_index: port_index,
                is_active: is_active,
                sample_size: sample_size
            },
        );

        let result = unsafe {
            clap_call! { audio_ports_activation=>set_active(plugin, is_input, port_index, is_active, sample_size) }
        };

        span.finish(record!(result: result));
        result
    }
}
