//! Abstractions for interacting with the `params` extension.

use anyhow::{Context, Result};
use clap_sys::events::{clap_input_events, clap_output_events};
use clap_sys::ext::params::{
    clap_param_info, clap_param_info_flags, clap_plugin_params, CLAP_EXT_PARAMS,
    CLAP_PARAM_IS_AUTOMATABLE, CLAP_PARAM_IS_AUTOMATABLE_PER_CHANNEL,
    CLAP_PARAM_IS_AUTOMATABLE_PER_KEY, CLAP_PARAM_IS_AUTOMATABLE_PER_NOTE_ID,
    CLAP_PARAM_IS_AUTOMATABLE_PER_PORT, CLAP_PARAM_IS_BYPASS, CLAP_PARAM_IS_MODULATABLE,
    CLAP_PARAM_IS_MODULATABLE_PER_CHANNEL, CLAP_PARAM_IS_MODULATABLE_PER_KEY,
    CLAP_PARAM_IS_MODULATABLE_PER_NOTE_ID, CLAP_PARAM_IS_MODULATABLE_PER_PORT,
    CLAP_PARAM_IS_STEPPED,
};
use clap_sys::id::clap_id;
use clap_sys::string_sizes::CLAP_NAME_SIZE;
use std::collections::BTreeMap;
use std::ffi::{c_void, CStr, CString};
use std::ops::RangeInclusive;
use std::pin::Pin;
use std::ptr::NonNull;

use super::Extension;
use crate::plugin::assert_plugin_state_lt;
use crate::plugin::instance::process::EventQueue;
use crate::plugin::instance::{Plugin, PluginStatus};
use crate::util::{self, c_char_slice_to_string, unsafe_clap_call};

pub type ParamInfo = BTreeMap<clap_id, Param>;

/// Abstraction for the `params` extension covering the main thread functionality.
#[derive(Debug)]
pub struct Params<'a> {
    plugin: &'a Plugin<'a>,
    params: NonNull<clap_plugin_params>,
}

impl<'a> Extension<&'a Plugin<'a>> for Params<'a> {
    const EXTENSION_ID: &'static CStr = CLAP_EXT_PARAMS;

    type Struct = clap_plugin_params;

    fn new(plugin: &'a Plugin<'a>, extension_struct: NonNull<Self::Struct>) -> Self {
        Self {
            plugin,
            params: extension_struct,
        }
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

impl Params<'_> {
    /// Used by the status assertion macros.
    fn status(&self) -> PluginStatus {
        self.plugin.status()
    }

    /// Get a parameter's value.
    pub fn get(&self, param_id: clap_id) -> Result<f64> {
        let params = self.params.as_ptr();
        let plugin = self.plugin.as_ptr();
        let mut value = 0.0f64;
        if unsafe_clap_call! { params=>get_value(plugin, param_id, &mut value) } {
            Ok(value)
        } else {
            anyhow::bail!(
                "'clap_plugin_params::get_value()' returned false for parameter ID {param_id}."
            );
        }
    }

    /// Convert a parameter value's to a string. Returns `Ok(None)` if the plugin doesn't support
    /// this, or an error if the returned string did not contain any null bytes or if it isn't
    /// invalid UTF-8.
    pub fn value_to_text(&self, param_id: clap_id, value: f64) -> Result<Option<String>> {
        let params = self.params.as_ptr();
        let plugin = self.plugin.as_ptr();
        let mut string_buffer = [0; CLAP_NAME_SIZE];
        if unsafe_clap_call! {
            params=>value_to_text(
                plugin,
                param_id,
                value,
                string_buffer.as_mut_ptr(),
                string_buffer.len() as u32,
            )
        } {
            c_char_slice_to_string(&string_buffer)
                .map(Some)
                .with_context(|| {
                    format!(
                        "Could not convert the string representation of {value} for parameter \
                         {param_id} to a UTF-8 string"
                    )
                })
        } else {
            Ok(None)
        }
    }

    /// Convert a string representation for a parameter to a value. Returns an `Ok(None)` if the
    /// plugin doesn't support this, or an error if the string contained internal null bytes.
    pub fn text_to_value(&self, param_id: clap_id, text: &str) -> Result<Option<f64>> {
        let text_cstring = CString::new(text)?;

        let params = self.params.as_ptr();
        let plugin = self.plugin.as_ptr();
        let mut value = 0.0f64;
        if unsafe_clap_call! {
            params=>text_to_value(
                plugin,
                param_id,
                text_cstring.as_ptr(),
                &mut value,
            )
        } {
            Ok(Some(value))
        } else {
            Ok(None)
        }
    }

    /// Get information about all of the plugin's parameters. Returns an error if the plugin's
    /// parameters are inconsistent. For instance, if there are multiple parameter with the same
    /// index, or if a parameter's minimum value is higher than the maximum value. This uses a
    /// BTreeMap to ensure the order is consistent between runs.
    pub fn info(&self) -> Result<ParamInfo> {
        let mut result = BTreeMap::new();

        let params = self.params.as_ptr();
        let plugin = self.plugin.as_ptr();
        let num_params = unsafe_clap_call! { params=>count(plugin) };

        // Right now this is only used to make sure the plugin doesn't have multiple bypass parameters
        let mut bypass_parameter_id = None;
        for i in 0..num_params {
            let mut info: clap_param_info = unsafe { std::mem::zeroed() };
            let success = unsafe_clap_call! { params=>get_info(plugin, i, &mut info) };
            if !success {
                anyhow::bail!(
                    "Plugin returned an error when querying parameter {i} ({num_params} total \
                     parameters)."
                );
            }

            let name = util::c_char_slice_to_string(&info.name).with_context(|| {
                format!(
                    "Could not read the name for parameter with stable ID {}",
                    info.id
                )
            })?;

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
                    "The module name for parameter '{}' (stable ID {}) starts with a leading \
                     slash: '{}'.",
                    &name,
                    info.id,
                    module
                )
            } else if module.ends_with('/') {
                anyhow::bail!(
                    "The module name for parameter '{}' (stable ID {}) ends with a trailing \
                     slash: '{}'.",
                    &name,
                    info.id,
                    module
                )
            } else if module.contains("//") {
                anyhow::bail!(
                    "The module name for parameter '{}' (stable ID {}) contains multiple \
                     subsequent slashes: '{}'.",
                    &name,
                    info.id,
                    module
                )
            }

            let range = info.min_value..=info.max_value;
            if info.min_value > info.max_value {
                anyhow::bail!(
                    "Parameter '{}' (stable ID {}) has a minimum value ({:?}) that's higher than \
                     it's maximum value ({:?}).",
                    &name,
                    info.id,
                    info.min_value,
                    info.max_value
                )
            }
            if !range.contains(&info.default_value) {
                anyhow::bail!(
                    "Parameter '{}' (stable ID {}) has a default value ({:?}) that falls outside \
                     of its value range ({:?}).",
                    &name,
                    info.id,
                    info.default_value,
                    &range
                )
            }
            if (info.flags & CLAP_PARAM_IS_STEPPED) != 0 {
                if info.min_value != info.min_value.trunc() {
                    anyhow::bail!(
                        "Parameter '{}' (stable ID {}) is a stepped parameter, but its minimum \
                         value ({:?}) is not an integer.",
                        &name,
                        info.id,
                        info.min_value,
                    )
                }
                if info.max_value != info.max_value.trunc() {
                    anyhow::bail!(
                        "Parameter '{}' (stable ID {}) is a stepped parameter, but its maximum \
                         value ({:?}) is not an integer.",
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
                        "Parameter '{}' (stable ID {}) is a bypass parameter, but it is not \
                         stepped.",
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
                    "Parameter '{}' (stable ID {}) is automatable per note ID, key, channel, or \
                     port, but does not have CLAP_PARAM_IS_AUTOMATABLE. This is likely a bug.",
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
                    "Parameter '{}' (stable ID {}) is modulatable per note ID, key, channel, or \
                     port, but does not have CLAP_PARAM_IS_MODULATABLE. This is likely a bug.",
                    &name,
                    info.id
                )
            }

            let processed_info = Param {
                name,
                cookie: info.cookie,
                range,
                default: info.default_value,
                flags: info.flags,
            };
            if result.insert(info.id, processed_info).is_some() {
                anyhow::bail!(
                    "The plugin contains multiple parameters with stable ID {}.",
                    info.id
                );
            }
        }

        Ok(result)
    }

    /// Perform a parameter flush.
    ///
    /// # Panics
    ///
    /// Panics if the plugin is active.
    pub fn flush(
        &self,
        input_events: &Pin<Box<EventQueue<clap_input_events>>>,
        output_events: &Pin<Box<EventQueue<clap_output_events>>>,
    ) {
        // This may only be called on the audio thread when the plugin is active. This object is the
        // main thread interface for the parameters extension.
        assert_plugin_state_lt!(self, PluginStatus::Activated);

        let params = self.params.as_ptr();
        let plugin = self.plugin.as_ptr();
        unsafe_clap_call! {
            params=>flush(
                plugin,
                input_events.vtable(),
                output_events.vtable(),
            )
        };
    }
}

impl Param {
    /// Whether this parameter is stepped.
    pub fn stepped(&self) -> bool {
        (self.flags & CLAP_PARAM_IS_STEPPED) != 0
    }
}
