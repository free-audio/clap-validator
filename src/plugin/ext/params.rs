//! Abstractions for interacting with the `params` extension.

use super::Extension;
use crate::cli::tracing::{Recordable, Recorder, Span, record};
use crate::plugin::instance::Plugin;
use crate::plugin::process::{InputEventQueue, OutputEventQueue};
use crate::plugin::util::{self, Proxy, c_char_slice_to_string, clap_call};
use anyhow::{Context, Result};
use clap_sys::ext::params::*;
use clap_sys::id::{CLAP_INVALID_ID, clap_id};
use clap_sys::string_sizes::CLAP_NAME_SIZE;
use std::collections::BTreeMap;
use std::ffi::{CStr, CString, c_void};
use std::ops::RangeInclusive;
use std::ptr::NonNull;

pub type ParamInfo = BTreeMap<clap_id, Param>;

/// Abstraction for the `params` extension covering the main thread functionality.
pub struct Params<'a> {
    plugin: &'a Plugin<'a>,
    params: NonNull<clap_plugin_params>,
}

impl<'a> Extension for Params<'a> {
    const IDS: &'static [&'static CStr] = &[CLAP_EXT_PARAMS];

    type Plugin = &'a Plugin<'a>;
    type Struct = clap_plugin_params;

    unsafe fn new(plugin: &'a Plugin<'a>, params: NonNull<Self::Struct>) -> Self {
        Self { plugin, params }
    }
}

/// Information about a parameter.
#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    /// This should be provided to the plugin when sending automation or modulation events for this
    /// parameter.
    pub cookie: *mut c_void,
    /// The parameter's value range.
    pub range: RangeInclusive<f64>,
    /// The parameter's default value.
    pub default: f64,
    /// The raw parameter flags bit field.
    pub flags: clap_param_info_flags,
}

unsafe impl Send for Param {}
unsafe impl Sync for Param {}

impl Params<'_> {
    /// Get a parameter's value.
    pub fn get(&self, param_id: clap_id) -> Result<f64> {
        let params = self.params.as_ptr();
        let plugin = self.plugin.as_ptr();
        let mut value = 0.0f64;

        let span = Span::begin("clap_plugin_params::get_value", record! { param_id: param_id });
        let result = unsafe {
            clap_call! { params=>get_value(plugin, param_id, &mut value) }
        };

        if result {
            span.finish(record!(result: value));
            Ok(value)
        } else {
            span.finish(record!(result: false));
            anyhow::bail!("'clap_plugin_params::get_value()' returned false for parameter ID {param_id}.");
        }
    }

    /// Convert a parameter value's to a string. Returns `Ok(None)` if the plugin doesn't support
    /// this, or an error if the returned string did not contain any null bytes or if it isn't
    /// invalid UTF-8.
    pub fn value_to_text(&self, param_id: clap_id, value: f64) -> Result<Option<String>> {
        let params = self.params.as_ptr();
        let plugin = self.plugin.as_ptr();

        let span = Span::begin(
            "clap_plugin_params::value_to_text",
            record! { param_id: param_id, value: value },
        );

        unsafe {
            let mut string_buffer = [0; CLAP_NAME_SIZE];
            let result = clap_call! {
                params=>value_to_text(
                    plugin,
                    param_id,
                    value,
                    string_buffer.as_mut_ptr(),
                    string_buffer.len() as u32,
                )
            };

            if result {
                match c_char_slice_to_string(&string_buffer) {
                    Ok(s) => {
                        span.finish(record!(result: &s));
                        Ok(Some(s))
                    }
                    Err(_) => {
                        span.finish(record!(result: "<invalid utf-8>"));
                        anyhow::bail!(
                            "The string representation of {value} for parameter {param_id} contains invalid UTF-8."
                        )
                    }
                }
            } else {
                Ok(None)
            }
        }
    }

    /// Convert a string representation for a parameter to a value. Returns an `Ok(None)` if the
    /// plugin doesn't support this, or an error if the string contained internal null bytes.
    pub fn text_to_value(&self, param_id: clap_id, text: &str) -> Result<Option<f64>> {
        let text_cstring = CString::new(text)?;

        let params = self.params.as_ptr();
        let plugin = self.plugin.as_ptr();

        let span = Span::begin(
            "clap_plugin_params::text_to_value",
            record! { param_id: param_id, text: text },
        );

        unsafe {
            let mut value = 0.0f64;
            let result = clap_call! {
                params=>text_to_value(
                    plugin,
                    param_id,
                    text_cstring.as_ptr(),
                    &mut value,
                )
            };

            if result {
                span.finish(record!(result: value));
                Ok(Some(value))
            } else {
                Ok(None)
            }
        }
    }

    /// Get information about all of the plugin's parameters. Returns an error if the plugin's
    /// parameters are inconsistent. For instance, if there are multiple parameter with the same
    /// index, or if a parameter's minimum value is higher than the maximum value. This uses a
    /// BTreeMap to ensure the order is consistent between runs.
    pub fn info(&self) -> Result<ParamInfo> {
        let mut result = BTreeMap::new();
        let num_params = self.get_raw_param_count();

        // Right now this is only used to make sure the plugin doesn't have multiple bypass parameters
        let mut bypass_parameter_id = None;
        for i in 0..num_params {
            let info = self.get_raw_param_info(i)?;

            if info.id == CLAP_INVALID_ID {
                anyhow::bail!("The stable ID for parameter {i} is `CLAP_INVALID_ID`.");
            }

            let name = util::c_char_slice_to_string(&info.name)
                .with_context(|| format!("Could not read the name for parameter with stable ID {}", info.id))?;

            // We don't use the module string, but we'll still check it for consistency. Basically
            // anything goes here as long as there are no trailing, leading, or multiple subsequent
            // slashes.
            let module = util::c_char_slice_to_string(&info.name).with_context(|| {
                format!(
                    "Could not read the module name for parameter '{}' (stable ID {})",
                    &name, info.id
                )
            })?;

            if module.starts_with('/') {
                anyhow::bail!(
                    "The module name for parameter '{}' (stable ID {}) starts with a leading slash: '{}'.",
                    &name,
                    info.id,
                    module
                )
            }

            if module.ends_with('/') {
                anyhow::bail!(
                    "The module name for parameter '{}' (stable ID {}) ends with a trailing slash: '{}'.",
                    &name,
                    info.id,
                    module
                )
            }

            if module.contains("//") {
                anyhow::bail!(
                    "The module name for parameter '{}' (stable ID {}) contains multiple subsequent slashes: '{}'.",
                    &name,
                    info.id,
                    module
                )
            }

            if info.min_value > info.max_value {
                anyhow::bail!(
                    "Parameter '{}' (stable ID {}) has a minimum value ({:?}) that's higher than it's maximum value \
                     ({:?}).",
                    &name,
                    info.id,
                    info.min_value,
                    info.max_value
                )
            }

            if !(info.min_value..=info.max_value).contains(&info.default_value) {
                anyhow::bail!(
                    "Parameter '{}' (stable ID {}) has a default value ({:?}) that falls outside of its value range \
                     ({:?}).",
                    &name,
                    info.id,
                    info.default_value,
                    info.min_value..=info.max_value
                )
            }

            if (info.flags & CLAP_PARAM_IS_STEPPED) != 0 {
                if info.min_value != info.min_value.trunc() {
                    anyhow::bail!(
                        "Parameter '{}' (stable ID {}) is a stepped parameter, but its minimum value ({:?}) is not an \
                         integer.",
                        &name,
                        info.id,
                        info.min_value,
                    )
                }
                if info.max_value != info.max_value.trunc() {
                    anyhow::bail!(
                        "Parameter '{}' (stable ID {}) is a stepped parameter, but its maximum value ({:?}) is not an \
                         integer.",
                        &name,
                        info.id,
                        info.max_value,
                    )
                }
            }

            if (info.flags & CLAP_PARAM_IS_BYPASS) != 0 {
                match bypass_parameter_id {
                    Some(bypass_parameter_id) => anyhow::bail!(
                        "The plugin has multiple bypass parameters (stable indices {} and {}).",
                        bypass_parameter_id,
                        info.id
                    ),
                    None => bypass_parameter_id = Some(info.id),
                }

                if (info.flags & CLAP_PARAM_IS_STEPPED) == 0 {
                    anyhow::bail!(
                        "Parameter '{}' (stable ID {}) is a bypass parameter, but it is not stepped.",
                        &name,
                        info.id
                    )
                }
            }

            // The last check here makes sure that per-X automatable or modulatable parameters are
            // also _just_ automatable/modulatable. This is technically allowed, but it is almost
            // certainly a bug.
            if (info.flags & CLAP_PARAM_IS_AUTOMATABLE) == 0
                && (info.flags
                    & (CLAP_PARAM_IS_AUTOMATABLE_PER_NOTE_ID
                        | CLAP_PARAM_IS_AUTOMATABLE_PER_KEY
                        | CLAP_PARAM_IS_AUTOMATABLE_PER_CHANNEL
                        | CLAP_PARAM_IS_AUTOMATABLE_PER_PORT))
                    != 0
            {
                anyhow::bail!(
                    "Parameter '{}' (stable ID {}) is automatable per note ID, key, channel, or port, but does not \
                     have CLAP_PARAM_IS_AUTOMATABLE. This is likely a bug.",
                    &name,
                    info.id
                )
            }

            if (info.flags & CLAP_PARAM_IS_MODULATABLE) == 0
                && (info.flags
                    & (CLAP_PARAM_IS_MODULATABLE_PER_NOTE_ID
                        | CLAP_PARAM_IS_MODULATABLE_PER_KEY
                        | CLAP_PARAM_IS_MODULATABLE_PER_CHANNEL
                        | CLAP_PARAM_IS_MODULATABLE_PER_PORT))
                    != 0
            {
                anyhow::bail!(
                    "Parameter '{}' (stable ID {}) is modulatable per note ID, key, channel, or port, but does not \
                     have CLAP_PARAM_IS_MODULATABLE. This is likely a bug.",
                    &name,
                    info.id
                )
            }

            if ((info.flags & CLAP_PARAM_IS_READONLY) != 0)
                && ((info.flags & CLAP_PARAM_IS_AUTOMATABLE) != 0 || (info.flags & CLAP_PARAM_IS_MODULATABLE) != 0)
            {
                anyhow::bail!(
                    "Parameter '{}' (stable ID {}) has the 'CLAP_PARAM_IS_READONLY' flag set, but it is also marked \
                     as automatable or modulatable. This is likely a bug.",
                    &name,
                    info.id
                )
            }

            let processed_info = Param {
                name,
                cookie: info.cookie,
                range: info.min_value..=info.max_value,
                default: info.default_value,
                flags: info.flags,
            };

            if result.insert(info.id, processed_info).is_some() {
                anyhow::bail!("The plugin contains multiple parameters with stable ID {}.", info.id);
            }
        }

        Ok(result)
    }

    /// Perform a parameter flush.
    pub fn flush(&self, input_events: &Proxy<InputEventQueue>, output_events: &Proxy<OutputEventQueue>) {
        // This may only be called on the audio thread when the plugin is active. This object is the
        // main thread interface for the parameters extension.
        self.plugin.status().assert_inactive();

        let params = self.params.as_ptr();
        let plugin = self.plugin.as_ptr();

        unsafe {
            let _span = Span::begin("clap_plugin_params::flush", ());
            clap_call! {
                params=>flush(
                    plugin,
                    Proxy::vtable(input_events),
                    Proxy::vtable(output_events),
                )
            };
        }
    }

    fn get_raw_param_count(&self) -> u32 {
        let params = self.params.as_ptr();
        let plugin = self.plugin.as_ptr();

        let span = Span::begin("clap_plugin_params::count", ());
        let result = unsafe {
            clap_call! { params=>count(plugin) }
        };

        span.finish(record!(result: result));
        result
    }

    fn get_raw_param_info(&self, index: u32) -> Result<clap_param_info> {
        let params = self.params.as_ptr();
        let plugin = self.plugin.as_ptr();

        let span = Span::begin("clap_plugin_params::get_info", record! { index: index });
        unsafe {
            let mut result = clap_param_info { ..std::mem::zeroed() };
            if !clap_call! { params=>get_info(plugin, index, &mut result) } {
                let num_params = self.get_raw_param_count();
                anyhow::bail!("Plugin returned false when querying parameter {index} ({num_params} total parameters).");
            }

            span.finish(record!(result: result));
            Ok(result)
        }
    }
}

impl Param {
    /// Whether the parameter is hidden and should be ignored.
    pub fn hidden(&self) -> bool {
        (self.flags & CLAP_PARAM_IS_HIDDEN) != 0
    }

    /// Whether the parameter is read-only and should not be changed.
    pub fn readonly(&self) -> bool {
        (self.flags & CLAP_PARAM_IS_READONLY) != 0
    }

    /// Whether this parameter is stepped.
    pub fn stepped(&self) -> bool {
        (self.flags & CLAP_PARAM_IS_STEPPED) != 0
    }

    /// Whether this parameter is automatable.
    pub fn automatable(&self) -> bool {
        (self.flags & CLAP_PARAM_IS_AUTOMATABLE) != 0
    }

    /// Whether this parameter is automatable per note ID, key, channel, or port.
    pub fn poly_automatable(&self) -> bool {
        (self.flags
            & (CLAP_PARAM_IS_AUTOMATABLE_PER_NOTE_ID
                | CLAP_PARAM_IS_AUTOMATABLE_PER_KEY
                | CLAP_PARAM_IS_AUTOMATABLE_PER_CHANNEL
                | CLAP_PARAM_IS_AUTOMATABLE_PER_PORT))
            != 0
    }

    /// Whether this parameter is modulatable.
    pub fn modulatable(&self) -> bool {
        (self.flags & CLAP_PARAM_IS_MODULATABLE) != 0
    }

    /// Whether this parameter is modulatable per note ID, key, channel, or port.
    pub fn poly_modulatable(&self) -> bool {
        (self.flags
            & (CLAP_PARAM_IS_MODULATABLE_PER_NOTE_ID
                | CLAP_PARAM_IS_MODULATABLE_PER_KEY
                | CLAP_PARAM_IS_MODULATABLE_PER_CHANNEL
                | CLAP_PARAM_IS_MODULATABLE_PER_PORT))
            != 0
    }
}

impl Recordable for clap_param_info {
    fn record(&self, record: &mut dyn Recorder) {
        record.record("id", self.id);

        record.record(
            "name",
            c_char_slice_to_string(&self.name).unwrap_or_else(|_| "<invalid utf-8>".to_string()),
        );

        record.record(
            "module",
            c_char_slice_to_string(&self.module).unwrap_or_else(|_| "<invalid utf-8>".to_string()),
        );

        record.record("cookie", format_args!("{:p}", self.cookie));
        record.record("min_value", self.min_value);
        record.record("max_value", self.max_value);
        record.record("default_value", self.default_value);

        record.record("flags.is_hidden", self.flags & CLAP_PARAM_IS_HIDDEN != 0);
        record.record("flags.is_readonly", self.flags & CLAP_PARAM_IS_READONLY != 0);
        record.record("flags.is_stepped", self.flags & CLAP_PARAM_IS_STEPPED != 0);
        record.record("flags.is_periodic", self.flags & CLAP_PARAM_IS_PERIODIC != 0);
        record.record("flags.is_bypass", self.flags & CLAP_PARAM_IS_BYPASS != 0);
        record.record("flags.is_enum", self.flags & CLAP_PARAM_IS_ENUM != 0);

        record.record(
            "flags.is_automatable.global",
            self.flags & CLAP_PARAM_IS_AUTOMATABLE != 0,
        );
        record.record(
            "flags.is_automatable.per_note_id",
            self.flags & CLAP_PARAM_IS_AUTOMATABLE_PER_NOTE_ID != 0,
        );
        record.record(
            "flags.is_automatable.per_key",
            self.flags & CLAP_PARAM_IS_AUTOMATABLE_PER_KEY != 0,
        );
        record.record(
            "flags.is_automatable.per_channel",
            self.flags & CLAP_PARAM_IS_AUTOMATABLE_PER_CHANNEL != 0,
        );
        record.record(
            "flags.is_automatable.per_port",
            self.flags & CLAP_PARAM_IS_AUTOMATABLE_PER_PORT != 0,
        );
        record.record(
            "flags.is_modulatable.global",
            self.flags & CLAP_PARAM_IS_MODULATABLE != 0,
        );
        record.record(
            "flags.is_modulatable.per_note_id",
            self.flags & CLAP_PARAM_IS_MODULATABLE_PER_NOTE_ID != 0,
        );
        record.record(
            "flags.is_modulatable.per_key",
            self.flags & CLAP_PARAM_IS_MODULATABLE_PER_KEY != 0,
        );
        record.record(
            "flags.is_modulatable.per_channel",
            self.flags & CLAP_PARAM_IS_MODULATABLE_PER_CHANNEL != 0,
        );
        record.record(
            "flags.is_modulatable.per_port",
            self.flags & CLAP_PARAM_IS_MODULATABLE_PER_PORT != 0,
        );

        record.record("flags.requires_process", self.flags & CLAP_PARAM_REQUIRES_PROCESS != 0);
    }
}
