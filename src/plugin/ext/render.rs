use crate::cli::tracing::{Span, record};
use crate::plugin::ext::Extension;
use crate::plugin::instance::Plugin;
use crate::plugin::util::clap_call;
use clap_sys::ext::render::*;
use std::ffi::CStr;
use std::ptr::NonNull;

pub struct Render<'a> {
    plugin: &'a Plugin<'a>,
    render: NonNull<clap_plugin_render>,
}

impl<'a> Extension for Render<'a> {
    const IDS: &'static [&'static CStr] = &[CLAP_EXT_RENDER];

    type Plugin = &'a Plugin<'a>;
    type Struct = clap_plugin_render;

    unsafe fn new(plugin: &'a Plugin<'a>, render: NonNull<Self::Struct>) -> Self {
        Self { plugin, render }
    }
}

impl<'a> Render<'a> {
    #[allow(unused)]
    pub fn has_hard_realtime_requirement(&self) -> bool {
        let render = self.render.as_ptr();
        let plugin = self.plugin.as_ptr();

        let span = Span::begin("clap_plugin_render::has_hard_realtime_requirement", ());
        let result = unsafe {
            clap_call! { render=>has_hard_realtime_requirement(plugin) }
        };

        span.finish(record!(result: result));
        result
    }

    pub fn set(&self, mode: RenderMode) -> bool {
        let render = self.render.as_ptr();
        let plugin = self.plugin.as_ptr();

        let span = Span::begin(
            "clap_plugin_render::set",
            record!(mode: match mode {
                RenderMode::Offline => "CLAP_RENDER_OFFLINE",
                RenderMode::Realtime => "CLAP_RENDER_REALTIME",
            }),
        );

        let result = unsafe {
            clap_call! { render=>set(plugin, match mode {
                RenderMode::Offline => CLAP_RENDER_OFFLINE,
                RenderMode::Realtime => CLAP_RENDER_REALTIME,
            }) }
        };

        span.finish(record!(result: result));
        result
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderMode {
    Offline,
    Realtime,
}
