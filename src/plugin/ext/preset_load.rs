//! Abstractions for interacting with the `preset-load` extension.

use anyhow::{Context, Result};
use clap_sys::ext::draft::preset_load::{clap_plugin_preset_load, CLAP_EXT_PRESET_LOAD};
use std::ffi::{CStr, CString};
use std::ptr::NonNull;

use crate::plugin::instance::Plugin;
use crate::plugin::preset_discovery::LocationValue;
use crate::util::unsafe_clap_call;

use super::Extension;

/// Abstraction for the `preset-load` extension covering the main thread functionality.
#[derive(Debug)]
pub struct PresetLoad<'a> {
    plugin: &'a Plugin<'a>,
    preset_load: NonNull<clap_plugin_preset_load>,
}

impl<'a> Extension<&'a Plugin<'a>> for PresetLoad<'a> {
    const EXTENSION_ID: &'static CStr = CLAP_EXT_PRESET_LOAD;

    type Struct = clap_plugin_preset_load;

    fn new(plugin: &'a Plugin<'a>, extension_struct: NonNull<Self::Struct>) -> Self {
        Self {
            plugin,
            preset_load: extension_struct,
        }
    }
}

impl PresetLoad<'_> {
    /// Try to load a preet based on a location and an optional load key. This information can be
    /// obtained through the preset discovery factory
    /// ([`Library::preset_discovery_factory()`][[crate::plugin::library::Library::preset_discovery_factory()]]).
    /// Load keys are only used for container presets, otherwise they're `None`. The semantics are
    /// similar to loading state.
    #[allow(clippy::wrong_self_convention)]
    pub fn from_location(&self, location: &LocationValue, load_key: Option<&str>) -> Result<()> {
        let (location_kind, location_ptr) = location.to_raw();
        let load_key_cstring = load_key
            .map(|load_key| {
                CString::new(load_key).context("Load key contained internal null bytes")
            })
            .transpose()?;

        let preset_load = self.preset_load.as_ptr();
        let plugin = self.plugin.as_ptr();
        let success = unsafe_clap_call! {
            preset_load=>from_location(
                plugin,
                location_kind,
                location_ptr,
                match load_key_cstring.as_ref() {
                    Some(load_key_cstring) => load_key_cstring.as_ptr(),
                    None => std::ptr::null(),
                }
            )
        };
        if success {
            Ok(())
        } else {
            anyhow::bail!(
                "'clap_plugin_preset_load::from_location()' returned false with {}{}.",
                location,
                match load_key {
                    Some(load_key) => format!(" and load key '{load_key}'"),
                    None => String::new(),
                },
            );
        }
    }
}
