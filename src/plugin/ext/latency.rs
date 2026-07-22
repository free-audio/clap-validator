//! Abstractions for interacting with the `latency` extension.

use anyhow::Result;
use clap_sys::ext::latency::{CLAP_EXT_LATENCY, clap_plugin_latency};
use std::ffi::CStr;
use std::ptr::NonNull;

use super::Extension;
use crate::plugin::assert_plugin_state_eq;
use crate::plugin::instance::{Plugin, PluginStatus};
use crate::util::unsafe_clap_call;

/// Abstraction for the `latency` extension.
#[derive(Debug)]
pub struct Latency<'a> {
    plugin: &'a Plugin<'a>,
    latency: NonNull<clap_plugin_latency>,
}

impl<'a> Extension<&'a Plugin<'a>> for Latency<'a> {
    const EXTENSION_ID: &'static CStr = CLAP_EXT_LATENCY;

    type Struct = clap_plugin_latency;

    fn new(plugin: &'a Plugin<'a>, extension_struct: NonNull<Self::Struct>) -> Self {
        Self {
            plugin,
            latency: extension_struct,
        }
    }
}

impl Latency<'_> {
    fn status(&self) -> PluginStatus {
        self.plugin.status()
    }

    /// Query the plugin's latency in samples. Requires the plugin to be activated.
    pub fn get(&self) -> Result<u32> {
        assert_plugin_state_eq!(self, PluginStatus::Activated);

        let latency = self.latency.as_ptr();
        let plugin = self.plugin.as_ptr();
        Ok(unsafe_clap_call! { latency=>get(plugin) })
    }
}
