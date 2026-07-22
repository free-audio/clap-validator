use crate::cli::tracing::{Span, record};
use crate::plugin::ext::Extension;
use crate::plugin::instance::Plugin;
use crate::plugin::util::clap_call;
use clap_sys::ext::surround::*;
use std::ffi::CStr;
use std::ptr::NonNull;

pub struct Surround<'a> {
    plugin: &'a Plugin<'a>,
    surround: NonNull<clap_plugin_surround>,
}

impl<'a> Extension for Surround<'a> {
    const IDS: &'static [&'static CStr] = &[CLAP_EXT_SURROUND, CLAP_EXT_SURROUND_COMPAT];

    type Plugin = &'a Plugin<'a>;
    type Struct = clap_plugin_surround;

    unsafe fn new(plugin: &'a Plugin<'a>, surround: NonNull<Self::Struct>) -> Self {
        Self { plugin, surround }
    }
}

impl<'a> Surround<'a> {
    pub fn is_channel_mask_supported(&self, channel_mask: u64) -> bool {
        let surround = self.surround.as_ptr();
        let plugin = self.plugin.as_ptr();

        let span = Span::begin(
            "clap_plugin_surround::is_channel_mask_supported",
            record! { channel_mask: channel_mask },
        );

        let result = unsafe {
            clap_call! {
                surround=>is_channel_mask_supported(
                    plugin,
                    channel_mask
                )
            }
        };

        span.finish(record!(result: result));
        result
    }

    pub fn get_channel_map(&self, is_input: bool, port_index: u32, channel_count: u32) -> Vec<u8> {
        let surround = self.surround.as_ptr();
        let plugin = self.plugin.as_ptr();

        let span = Span::begin(
            "clap_plugin_surround::get_channel_map",
            record! {
                is_input: is_input,
                port_index: port_index,
                channel_count: channel_count
            },
        );

        unsafe {
            let mut channel_map = vec![0u8; channel_count as usize];
            let channels_real = clap_call! {
                surround=>get_channel_map(
                    plugin,
                    is_input,
                    port_index,
                    channel_map.as_mut_ptr(),
                    channel_count
                )
            };

            channel_map.truncate(channels_real as usize);
            span.finish(record! { channel_map: format_args!("{:?}", channel_map) });
            channel_map
        }
    }
}
