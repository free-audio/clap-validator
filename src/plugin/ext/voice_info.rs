use crate::cli::tracing::{Recordable, Recorder, Span, record};
use crate::plugin::ext::Extension;
use crate::plugin::instance::Plugin;
use crate::plugin::util::clap_call;
use clap_sys::ext::voice_info::*;
use std::ffi::CStr;
use std::mem::zeroed;
use std::ptr::NonNull;

#[allow(unused)]
pub struct VoiceInfo<'a> {
    plugin: &'a Plugin<'a>,
    voice_info: NonNull<clap_plugin_voice_info>,
}

impl<'a> Extension for VoiceInfo<'a> {
    const IDS: &'static [&'static CStr] = &[CLAP_EXT_VOICE_INFO];

    type Plugin = &'a Plugin<'a>;
    type Struct = clap_plugin_voice_info;

    unsafe fn new(plugin: &'a Plugin<'a>, voice_info: NonNull<Self::Struct>) -> Self {
        Self { plugin, voice_info }
    }
}

impl<'a> VoiceInfo<'a> {
    pub fn get(&self) -> Option<clap_voice_info> {
        self.plugin.status().assert_active();

        let voice_info = self.voice_info.as_ptr();
        let plugin = self.plugin.as_ptr();

        let span = Span::begin("clap_plugin_voice_info::get", ());

        unsafe {
            let mut result = clap_voice_info { ..zeroed() };
            if clap_call! { voice_info=>get(plugin, &mut result) } {
                span.finish(record!(result: result));
                Some(result)
            } else {
                span.finish(record!(result: false));
                None
            }
        }
    }
}

impl Recordable for clap_voice_info {
    fn record(&self, record: &mut dyn Recorder) {
        record.record("voice_count", self.voice_count);
        record.record("voice_capacity", self.voice_capacity);
        record.record(
            "supports_overlapping_notes",
            self.flags & CLAP_VOICE_INFO_SUPPORTS_OVERLAPPING_NOTES != 0,
        );
    }
}
