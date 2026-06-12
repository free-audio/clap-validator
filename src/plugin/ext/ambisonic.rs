use crate::cli::tracing::{Recordable, Recorder, Span, record};
use crate::plugin::ext::Extension;
use crate::plugin::instance::Plugin;
use crate::plugin::util::clap_call;
use clap_sys::ext::ambisonic::*;
use std::ffi::CStr;
use std::mem::zeroed;
use std::ptr::NonNull;

pub struct Ambisonic<'a> {
    plugin: &'a Plugin<'a>,
    ambisonic: NonNull<clap_plugin_ambisonic>,
}

impl<'a> Extension for Ambisonic<'a> {
    const IDS: &'static [&'static CStr] = &[CLAP_EXT_AMBISONIC, CLAP_EXT_AMBISONIC_COMPAT];

    type Plugin = &'a Plugin<'a>;
    type Struct = clap_plugin_ambisonic;

    unsafe fn new(plugin: &'a Plugin<'a>, ambisonic: NonNull<Self::Struct>) -> Self {
        Self { plugin, ambisonic }
    }
}

impl<'a> Ambisonic<'a> {
    pub fn is_config_supported(&self, config: &clap_ambisonic_config) -> bool {
        let ambisonic = self.ambisonic.as_ptr();
        let plugin = self.plugin.as_ptr();

        let span = Span::begin("clap_plugin_ambisonic::is_config_supported", config);
        let result = unsafe {
            clap_call! { ambisonic=>is_config_supported(plugin, config) }
        };

        span.finish(record!(result: result));
        result
    }

    pub fn get_config(&self, is_input: bool, port_index: u32) -> Option<clap_ambisonic_config> {
        let ambisonic = self.ambisonic.as_ptr();
        let plugin = self.plugin.as_ptr();

        let span = Span::begin(
            "clap_plugin_ambisonic::get_config",
            record! {
                is_input: is_input,
                port_index: port_index
            },
        );

        unsafe {
            let mut config = clap_ambisonic_config { ..zeroed() };
            let result = clap_call! { ambisonic=>get_config(plugin, is_input, port_index, &mut config) };

            if result {
                span.finish(record!(result: config));
                Some(config)
            } else {
                span.finish(record!(result: false));
                None
            }
        }
    }
}

impl Recordable for clap_ambisonic_config {
    fn record(&self, record: &mut dyn Recorder) {
        record.record(
            "ordering",
            match self.ordering {
                CLAP_AMBISONIC_ORDERING_ACN => "CLAP_AMBISONIC_ORDERING_ACN",
                CLAP_AMBISONIC_ORDERING_FUMA => "CLAP_AMBISONIC_ORDERING_FUMA",
                _ => "?",
            },
        );

        record.record(
            "normalization",
            match self.normalization {
                CLAP_AMBISONIC_NORMALIZATION_MAXN => "CLAP_AMBISONIC_NORMALIZATION_MAXN",
                CLAP_AMBISONIC_NORMALIZATION_SN3D => "CLAP_AMBISONIC_NORMALIZATION_SN3D",
                CLAP_AMBISONIC_NORMALIZATION_N3D => "CLAP_AMBISONIC_NORMALIZATION_N3D",
                CLAP_AMBISONIC_NORMALIZATION_SN2D => "CLAP_AMBISONIC_NORMALIZATION_SN2D",
                CLAP_AMBISONIC_NORMALIZATION_N2D => "CLAP_AMBISONIC_NORMALIZATION_N2D",
                _ => "?",
            },
        );
    }
}
