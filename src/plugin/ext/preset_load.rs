//! Abstractions for interacting with the `preset-load` extension.

use anyhow::{Context, Result};
use clap_sys::ext::preset_load::{CLAP_EXT_PRESET_LOAD, clap_plugin_preset_load};
use std::ffi::{CStr, CString};
use std::ptr::NonNull;

use super::Extension;
use crate::cli::tracing::{Span, record};
use crate::plugin::instance::Plugin;
use crate::plugin::preset_discovery::LocationValue;
use crate::plugin::util::clap_call;

/// Abstraction for the `preset-load` extension covering the main thread functionality.
pub struct PresetLoad<'a> {
    plugin: &'a Plugin<'a>,
    preset_load: NonNull<clap_plugin_preset_load>,
}

impl<'a> Extension for PresetLoad<'a> {
    const IDS: &'static [&'static CStr] = &[CLAP_EXT_PRESET_LOAD];

    type Plugin = &'a Plugin<'a>;
    type Struct = clap_plugin_preset_load;

    unsafe fn new(plugin: &'a Plugin<'a>, preset_load: NonNull<Self::Struct>) -> Self {
        Self { plugin, preset_load }
    }
}

impl PresetLoad<'_> {
    /// Try to load a preset based on a location and an optional load key. This information can be
    /// obtained through the preset discovery factory
    /// ([`Library::preset_discovery_factory()`][[crate::plugin::library::Library::preset_discovery_factory()]]).
    /// Load keys are only used for container presets, otherwise they're `None`. The semantics are
    /// similar to loading state.
    pub fn load_from_location(&self, location: &LocationValue, load_key: Option<&str>) -> Result<()> {
        let (location_kind, location_ptr) = location.to_raw();
        let load_key_cstring = load_key
            .map(|load_key| CString::new(load_key).context("Load key contained internal null bytes"))
            .transpose()?;

        let preset_load = self.preset_load.as_ptr();
        let plugin = self.plugin.as_ptr();

        let span = Span::begin(
            "clap_plugin_preset_load::from_location",
            record! {
                location: location,
                load_key: load_key
            },
        );

        let result = unsafe {
            clap_call! {
                preset_load=>from_location(
                    plugin,
                    location_kind,
                    location_ptr,
                    match load_key_cstring.as_ref() {
                        Some(load_key_cstring) => load_key_cstring.as_ptr(),
                        None => std::ptr::null(),
                    }
                )
            }
        };

        span.finish(record!(result: result));

        if result {
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
