//! Abstractions for interacting with the `note-ports` extension.

use anyhow::Result;
use clap_sys::ext::note_ports::{
    clap_note_dialect, clap_note_port_info, clap_plugin_note_ports, CLAP_EXT_NOTE_PORTS,
};
use std::mem;
use std::os::raw::c_char;
use std::ptr::NonNull;

use crate::plugin::instance::Plugin;

use super::Extension;

/// Abstraction for the `note-ports` extension covering the main thread functionality.
#[derive(Debug)]
pub struct NotePorts<'a> {
    plugin: &'a Plugin<'a>,
    note_ports: NonNull<clap_plugin_note_ports>,
}

/// The note port configuration for a plugin.
#[derive(Debug, Default)]
pub struct NotePortConfig {
    /// Configuration for the plugin's input note ports.
    pub inputs: Vec<NotePort>,
    /// Configuration for the plugin's output note ports.
    pub outputs: Vec<NotePort>,
}

/// The configuration for a single note port.
#[derive(Debug)]
pub struct NotePort {
    /// The preferred dialect for this note port.
    pub prefered_dialect: clap_note_dialect,
    /// All supported note dialects for this port.
    pub supported_dialects: Vec<clap_note_dialect>,
}

impl<'a> Extension<&'a Plugin<'a>> for NotePorts<'a> {
    const EXTENSION_ID: *const c_char = CLAP_EXT_NOTE_PORTS;

    type Struct = clap_plugin_note_ports;

    fn new(plugin: &'a Plugin<'a>, extension_struct: NonNull<Self::Struct>) -> Self {
        Self {
            plugin,
            note_ports: extension_struct,
        }
    }
}

impl NotePorts<'_> {
    /// Get the note port configuration for this plugin. This also checks whether the dialect types
    /// are consistent.
    pub fn config(&self) -> Result<NotePortConfig> {
        let mut config = NotePortConfig::default();

        let note_ports = unsafe { self.note_ports.as_ref() };
        let num_inputs = unsafe { (note_ports.count)(self.plugin.as_ptr(), true) };
        let num_outputs = unsafe { (note_ports.count)(self.plugin.as_ptr(), false) };

        for i in 0..num_inputs {
            let mut info: clap_note_port_info = unsafe { std::mem::zeroed() };
            let success = unsafe { (note_ports.get)(self.plugin.as_ptr(), 0, true, &mut info) };
            if !success {
                anyhow::bail!("Plugin returned an error when querying input note port {i} ({num_inputs} total input ports)");
            }

            let num_preferred_dialects = info.preferred_dialect.count_ones();
            if num_preferred_dialects != 1 {
                anyhow::bail!(
                    "Plugin prefers {num_preferred_dialects} dialects for input note port {i}"
                );
            }

            if (info.supported_dialects & info.preferred_dialect) == 0 {
                anyhow::bail!("Plugin prefers note dialect {:#b} for input note port {i} which is not contained within the supported note dialects field ({:#b})", info.preferred_dialect, info.supported_dialects);
            }

            config.inputs.push(NotePort {
                prefered_dialect: info.preferred_dialect,
                supported_dialects: (0..(mem::size_of::<clap_note_dialect>() * 8) - 1)
                    .map(|bit| 1 << bit)
                    .filter(|flag| (info.supported_dialects & flag) != 0)
                    .collect(),
            });
        }

        for i in 0..num_outputs {
            let mut info: clap_note_port_info = unsafe { std::mem::zeroed() };
            let success = unsafe { (note_ports.get)(self.plugin.as_ptr(), 0, true, &mut info) };
            if !success {
                anyhow::bail!("Plugin returned an error when querying output note port {i} ({num_outputs} total output ports)");
            }

            let num_preferred_dialects = info.preferred_dialect.count_ones();
            if num_preferred_dialects != 1 {
                anyhow::bail!(
                    "Plugin prefers {num_preferred_dialects} dialects for output note port {i}"
                );
            }

            if (info.supported_dialects & info.preferred_dialect) == 0 {
                anyhow::bail!("Plugin prefers note dialect {:#b} for output note port {i} which is not contained within the supported note dialects field ({:#b})", info.preferred_dialect, info.supported_dialects);
            }

            config.outputs.push(NotePort {
                prefered_dialect: info.preferred_dialect,
                supported_dialects: (0..(mem::size_of::<clap_note_dialect>() * 8) - 1)
                    .map(|bit| 1 << bit)
                    .filter(|flag| (info.supported_dialects & flag) != 0)
                    .collect(),
            });
        }

        Ok(config)
    }
}
