//! Abstractions for interacting with the `tail` extension.

use anyhow::Result;
use clap_sys::ext::tail::{CLAP_EXT_TAIL, clap_plugin_tail};
use std::ffi::CStr;
use std::ptr::NonNull;

use super::Extension;
use crate::plugin::assert_plugin_state_eq;
use crate::plugin::instance::{Plugin, PluginStatus};
use crate::util::unsafe_clap_call;

/// Abstraction for the `tail` extension.
#[derive(Debug)]
pub struct Tail<'a> {
    plugin: &'a Plugin<'a>,
    tail: NonNull<clap_plugin_tail>,
}

impl<'a> Extension<&'a Plugin<'a>> for Tail<'a> {
    const EXTENSION_ID: &'static CStr = CLAP_EXT_TAIL;

    type Struct = clap_plugin_tail;

    fn new(plugin: &'a Plugin<'a>, extension_struct: NonNull<Self::Struct>) -> Self {
        Self {
            plugin,
            tail: extension_struct,
        }
    }
}

impl Tail<'_> {
    fn status(&self) -> PluginStatus {
        self.plugin.status()
    }

    /// Query the processing tail length in samples. Requires the plugin to be activated.
    pub fn get(&self) -> Result<u32> {
        assert_plugin_state_eq!(self, PluginStatus::Activated);

        let tail = self.tail.as_ptr();
        let plugin = self.plugin.as_ptr();
        Ok(unsafe_clap_call! { tail=>get(plugin) })
    }
}
