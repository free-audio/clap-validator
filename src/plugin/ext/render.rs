//! Abstractions for interacting with the `render` extension.

use anyhow::Result;
use clap_sys::ext::render::{
    CLAP_EXT_RENDER, CLAP_RENDER_OFFLINE, CLAP_RENDER_REALTIME, clap_plugin_render,
    clap_plugin_render_mode,
};
use std::ffi::CStr;
use std::ptr::NonNull;

use super::Extension;
use crate::plugin::assert_plugin_state_initialized;
use crate::plugin::instance::{Plugin, PluginStatus};
use crate::util::unsafe_clap_call;

/// Abstraction for the `render` extension.
#[derive(Debug)]
pub struct Render<'a> {
    plugin: &'a Plugin<'a>,
    render: NonNull<clap_plugin_render>,
}

impl<'a> Extension<&'a Plugin<'a>> for Render<'a> {
    const EXTENSION_ID: &'static CStr = CLAP_EXT_RENDER;

    type Struct = clap_plugin_render;

    fn new(plugin: &'a Plugin<'a>, extension_struct: NonNull<Self::Struct>) -> Self {
        Self {
            plugin,
            render: extension_struct,
        }
    }
}

impl Render<'_> {
    fn status(&self) -> PluginStatus {
        self.plugin.status()
    }

    /// Whether the plugin has a hard realtime requirement.
    pub fn has_hard_realtime_requirement(&self) -> Result<bool> {
        assert_plugin_state_initialized!(self);

        let render = self.render.as_ptr();
        let plugin = self.plugin.as_ptr();
        Ok(unsafe_clap_call! { render=>has_hard_realtime_requirement(plugin) })
    }

    /// Request a render mode. Returns `false` if the plugin does not support the mode.
    pub fn set(&self, mode: clap_plugin_render_mode) -> Result<bool> {
        assert_plugin_state_initialized!(self);

        let render = self.render.as_ptr();
        let plugin = self.plugin.as_ptr();
        Ok(unsafe_clap_call! { render=>set(plugin, mode) })
    }

    /// Convenience: set realtime mode.
    pub fn set_realtime(&self) -> Result<bool> {
        self.set(CLAP_RENDER_REALTIME)
    }

    /// Convenience: set offline mode.
    pub fn set_offline(&self) -> Result<bool> {
        self.set(CLAP_RENDER_OFFLINE)
    }
}
