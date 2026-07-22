use crate::cli::tracing::{Span, record};
use crate::plugin::ext::Extension;
use crate::plugin::instance::PluginAudioThread;
use crate::plugin::util::clap_call;
use clap_sys::ext::tail::{CLAP_EXT_TAIL, clap_plugin_tail};
use std::ffi::CStr;
use std::ptr::NonNull;

pub struct Tail<'a> {
    plugin: &'a PluginAudioThread<'a>,
    tail: NonNull<clap_plugin_tail>,
}

impl<'a> Extension for Tail<'a> {
    const IDS: &'static [&'static CStr] = &[CLAP_EXT_TAIL];

    type Plugin = &'a PluginAudioThread<'a>;
    type Struct = clap_plugin_tail;

    unsafe fn new(plugin: &'a PluginAudioThread<'a>, tail: NonNull<Self::Struct>) -> Self {
        Self { plugin, tail }
    }
}

impl<'a> Tail<'a> {
    pub fn get(&self) -> u32 {
        let tail = self.tail.as_ptr();
        let plugin = self.plugin.as_ptr();

        let span = Span::begin("clap_plugin_tail::get", ());
        let result = unsafe {
            clap_call! { tail=>get(plugin) }
        };

        span.finish(record!(result: result));
        result
    }
}
