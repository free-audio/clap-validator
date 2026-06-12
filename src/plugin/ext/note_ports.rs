//! Abstractions for interacting with the `note-ports` extension.

use super::Extension;
use crate::cli::tracing::{Recordable, Recorder, Span, record};
use crate::plugin::instance::Plugin;
use crate::plugin::util::clap_call;
use anyhow::{Context, Result};
use clap_sys::ext::note_ports::*;
use clap_sys::id::CLAP_INVALID_ID;
use std::collections::HashSet;
use std::ffi::CStr;
use std::mem;
use std::ptr::NonNull;

/// Abstraction for the `note-ports` extension covering the main thread functionality.
pub struct NotePorts<'a> {
    plugin: &'a Plugin<'a>,
    note_ports: NonNull<clap_plugin_note_ports>,
}

/// The note port configuration for a plugin.
#[derive(Debug, Clone, Default)]
pub struct NotePortConfig {
    /// Configuration for the plugin's input note ports.
    pub inputs: Vec<NotePort>,
    /// Configuration for the plugin's output note ports.
    pub outputs: Vec<NotePort>,
}

/// The configuration for a single note port.
#[derive(Debug, Clone)]
pub struct NotePort {
    /// All supported note dialects for this port. All of these note dialect values will only ever
    /// contain a single value.
    pub supported_dialects: Vec<clap_note_dialect>,
}

impl<'a> Extension for NotePorts<'a> {
    const IDS: &'static [&'static CStr] = &[CLAP_EXT_NOTE_PORTS];

    type Plugin = &'a Plugin<'a>;
    type Struct = clap_plugin_note_ports;

    unsafe fn new(plugin: &'a Plugin<'a>, note_ports: NonNull<Self::Struct>) -> Self {
        Self { plugin, note_ports }
    }
}

impl NotePorts<'_> {
    /// Get the note port configuration for this plugin. This also checks whether the dialect types
    /// are consistent.
    pub fn config(&self) -> Result<NotePortConfig> {
        let mut config = NotePortConfig::default();

        let num_inputs = self.get_raw_port_count(true);
        let num_outputs = self.get_raw_port_count(false);

        // We don't need the port's stable IDs, but we'll still verify that they're unique
        let mut input_stable_indices: HashSet<u32> = HashSet::new();
        let mut output_stable_indices: HashSet<u32> = HashSet::new();

        for index in 0..num_inputs {
            let info = self.get_raw_port_info(true, index)?;

            if !input_stable_indices.insert(info.id) {
                anyhow::bail!("The stable ID of input note port {index} ({}) is a duplicate.", info.id);
            }

            config.inputs.push(
                check_note_port_valid(&info)
                    .with_context(|| format!("Inconsistent port info for input note port {index}"))?,
            );
        }

        for index in 0..num_outputs {
            let info = self.get_raw_port_info(false, index)?;

            if !output_stable_indices.insert(info.id) {
                anyhow::bail!(
                    "The stable ID of output note port {index} ({}) is a duplicate.",
                    info.id
                );
            }

            config.outputs.push(
                check_note_port_valid(&info)
                    .with_context(|| format!("Inconsistent port info for output note port {index}"))?,
            );
        }

        Ok(config)
    }

    fn get_raw_port_count(&self, is_input: bool) -> u32 {
        let note_ports = self.note_ports.as_ptr();
        let plugin = self.plugin.as_ptr();

        let span = Span::begin(
            "clap_plugin_note_ports::count",
            record! {
                is_input: is_input
            },
        );

        let result = unsafe {
            clap_call! { note_ports=>count(plugin, is_input) }
        };

        span.finish(record!(result: result));
        result
    }

    fn get_raw_port_info(&self, is_input: bool, port_index: u32) -> Result<clap_note_port_info> {
        let note_ports = self.note_ports.as_ptr();
        let plugin = self.plugin.as_ptr();

        let span = Span::begin(
            "clap_plugin_note_ports::get",
            record! {
                is_input: is_input,
                port_index: port_index
            },
        );

        unsafe {
            let mut info = clap_note_port_info { ..std::mem::zeroed() };
            let result = clap_call! { note_ports=>get(plugin, port_index, is_input, &mut info) };
            if result {
                span.finish(record!(result: info));
                Ok(info)
            } else {
                span.finish(record!(result: false));
                anyhow::bail!(
                    "Plugin returned false when querying {} note port {port_index} ({} total {} ports).",
                    if is_input { "input" } else { "output" },
                    self.get_raw_port_count(is_input),
                    if is_input { "input" } else { "output" }
                );
            }
        }
    }
}

impl NotePort {
    pub fn supports_clap(&self) -> bool {
        self.supported_dialects.contains(&CLAP_NOTE_DIALECT_CLAP)
    }

    pub fn supports_midi(&self) -> bool {
        self.supported_dialects.contains(&CLAP_NOTE_DIALECT_MIDI)
            || self.supported_dialects.contains(&CLAP_NOTE_DIALECT_MIDI_MPE)
    }
}

fn check_note_port_valid(info: &clap_note_port_info) -> Result<NotePort> {
    if info.id == CLAP_INVALID_ID {
        anyhow::bail!("The stable ID is `CLAP_INVALID_ID`.");
    }

    let num_preferred_dialects = info.preferred_dialect.count_ones();
    if num_preferred_dialects != 1 {
        anyhow::bail!(
            "`preferred_dialect` contains multiple ({num_preferred_dialects}) dialect values, must be exactly one."
        );
    }

    if (info.supported_dialects & info.preferred_dialect) == 0 {
        anyhow::bail!(
            "Port prefers note dialect {:#b} which is not contained within the supported note dialects field ({:#b}).",
            info.preferred_dialect,
            info.supported_dialects
        );
    }

    Ok(NotePort {
        supported_dialects: (0..(mem::size_of::<clap_note_dialect>() * 8) - 1)
            .map(|bit| 1 << bit)
            .filter(|flag| (info.supported_dialects & flag) != 0)
            .collect(),
    })
}

impl Recordable for clap_note_port_info {
    fn record(&self, record: &mut dyn Recorder) {
        record.record("id", self.id);

        record.record(
            "supported_dialects.clap",
            self.supported_dialects & CLAP_NOTE_DIALECT_CLAP != 0,
        );
        record.record(
            "supported_dialects.midi",
            self.supported_dialects & CLAP_NOTE_DIALECT_MIDI != 0,
        );
        record.record(
            "supported_dialects.midi_mpe",
            self.supported_dialects & CLAP_NOTE_DIALECT_MIDI_MPE != 0,
        );
        record.record(
            "supported_dialects.midi2",
            self.supported_dialects & CLAP_NOTE_DIALECT_MIDI2 != 0,
        );

        record.record(
            "preferred_dialect.clap",
            self.preferred_dialect & CLAP_NOTE_DIALECT_CLAP != 0,
        );
        record.record(
            "preferred_dialect.midi",
            self.preferred_dialect & CLAP_NOTE_DIALECT_MIDI != 0,
        );
        record.record(
            "preferred_dialect.midi_mpe",
            self.preferred_dialect & CLAP_NOTE_DIALECT_MIDI_MPE != 0,
        );
        record.record(
            "preferred_dialect.midi2",
            self.preferred_dialect & CLAP_NOTE_DIALECT_MIDI2 != 0,
        );
    }
}
