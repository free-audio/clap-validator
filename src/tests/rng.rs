//! Utilities for generating pseudo-random data.

use anyhow::{Context, Result};
use clap_sys::events::{
    clap_event_header, clap_event_note, CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_NOTE_OFF,
    CLAP_EVENT_NOTE_ON,
};
use clap_sys::ext::note_ports::{
    CLAP_NOTE_DIALECT_CLAP, CLAP_NOTE_DIALECT_MIDI, CLAP_NOTE_DIALECT_MIDI_MPE,
};
use rand::Rng;
use rand_pcg::Pcg32;

use crate::plugin::{audio_thread::process::Event, ext::note_ports::NotePortConfig};

/// Create a new pseudo-random number generator with a fixed seed.
pub fn new_prng() -> Pcg32 {
    Pcg32::new(1337, 420)
}

/// A random note and MIDI event generator that generates consistent events based on the
/// capabilities stored in a [`NotePortConfig`]
#[derive(Debug, Clone)]
pub struct NoteGenerator {
    config: NotePortConfig,
    /// Contains the currently playing notes per-port. We'll be nice and not send overlapping notes
    /// or note-offs without a corresponding note-on.
    ///
    /// TODO: Do send overlapping notes with different note IDs if the plugin claims to support it.
    active_notes: Vec<Vec<Note>>,
    /// The CLAP note ID for the next note on event.
    next_note_id: i32,
}

/// The description of an active note in the [`NoteGenerator`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Note {
    pub key: i16,
    pub channel: i16,
    pub note_id: i32,
}

/// The different kinds of events we can generate. The event type chosen depends on the plugin.
#[derive(Debug, Clone, Copy)]
enum NoteEventType {
    ClapNoteOn,
    ClapNoteOff,
    ClapNoteChoke,
    ClapNoteExpression,
    MidiNoteOn,
    MidiNoteOff,
    MidiChannelPressure,
    MidiPolyKeyPressure,
    MidiPitchBend,
    MidiCc,
    MidiProgramChange,
}

impl NoteGenerator {
    /// Create a new random note generator based on a plugin's note port configuration.
    pub fn new(config: NotePortConfig) -> Self {
        let num_inputs = config.inputs.len();

        NoteGenerator {
            config,
            active_notes: vec![Vec::new(); num_inputs],
            next_note_id: 0,
        }
    }

    /// Generate a random note event for one of the plugin's note ports depending on the port's
    /// capabilities. Returns an error if the plugin doesn't have any note ports or if the note
    /// ports don't support either MIDI or CLAP note events.
    pub fn generate(&mut self, prng: &mut Pcg32, time_offset: u32) -> Result<Event> {
        if self.config.inputs.is_empty() {
            anyhow::bail!("Cannot generate note events for a plugin with no input note ports");
        }

        // We'll ignore the prefered note dialect and pick from all of the supported note dialects.
        // The plugin may get a CLAP note on and a MIDI note off if it supports both of those things
        let note_port_idx = prng.gen_range(0..self.config.inputs.len());
        let supports_clap_note_events = self.config.inputs[note_port_idx]
            .supported_dialects
            .contains(&CLAP_NOTE_DIALECT_CLAP);
        let supports_midi_events = self.config.inputs[note_port_idx]
            .supported_dialects
            .contains(&CLAP_NOTE_DIALECT_MIDI)
            || self.config.inputs[note_port_idx]
                .supported_dialects
                .contains(&CLAP_NOTE_DIALECT_MIDI_MPE);
        let possible_events =
            NoteEventType::supported_types(supports_clap_note_events, supports_midi_events)
                .with_context(|| format!("Note input port {note_port_idx} supports neither CLAP note events nor MIDI. This is technically allowed, but few hosts will be able to interact with the plugin."))?;

        // We could do this in a smarter way to avoid generating impossible event types (like a note
        // off when there are no active notes), but this should work fine.
        for _ in 0..1024 {
            let event_type = prng.sample(rand::distributions::Slice::new(possible_events).unwrap());
            match event_type {
                NoteEventType::ClapNoteOn => {
                    let key = prng.gen_range(0..128);
                    let channel = prng.gen_range(0..16);
                    let note_id = self.next_note_id;
                    let note = Note {
                        key,
                        channel,
                        note_id,
                    };
                    if self.active_notes[note_port_idx].contains(&note) {
                        continue;
                    }
                    self.active_notes[note_port_idx].push(note);
                    self.next_note_id = self.next_note_id.wrapping_add(1);

                    // TODO: Generate the note event, do the same thing for the other events
                    let velocity = prng.gen_range(0.0..=1.0);
                    return Ok(Event::ClapNoteEvent(clap_event_note {
                        header: clap_event_header {
                            size: std::mem::size_of::<clap_event_note>() as u32,
                            time: time_offset,
                            space_id: CLAP_CORE_EVENT_SPACE_ID,
                            type_: CLAP_EVENT_NOTE_ON,
                            // TODO: There's a live flag here, should we also randomize this?
                            flags: 0,
                        },
                        note_id,
                        port_index: note_port_idx as i16,
                        channel,
                        key,
                        velocity,
                    }));
                }
                NoteEventType::ClapNoteOff => {
                    if self.active_notes[note_port_idx].is_empty() {
                        continue;
                    }

                    let note_idx = prng.gen_range(0..self.active_notes[note_port_idx].len());
                    let note = self.active_notes[note_port_idx].remove(note_idx);

                    // TODO: Generate the note event, do the same thing for the other events
                    let velocity = prng.gen_range(0.0..=1.0);
                    return Ok(Event::ClapNoteEvent(clap_event_note {
                        header: clap_event_header {
                            size: std::mem::size_of::<clap_event_note>() as u32,
                            time: time_offset,
                            space_id: CLAP_CORE_EVENT_SPACE_ID,
                            type_: CLAP_EVENT_NOTE_OFF,
                            flags: 0,
                        },
                        note_id: note.note_id,
                        port_index: note_port_idx as i16,
                        channel: note.channel,
                        key: note.key,
                        velocity,
                    }));
                }
                // TODO: Implement the rest of these events
                NoteEventType::ClapNoteChoke => todo!(),
                NoteEventType::ClapNoteExpression => todo!(),
                NoteEventType::MidiNoteOn => todo!(),
                NoteEventType::MidiNoteOff => todo!(),
                NoteEventType::MidiChannelPressure => todo!(),
                NoteEventType::MidiPolyKeyPressure => todo!(),
                NoteEventType::MidiPitchBend => todo!(),
                NoteEventType::MidiCc => todo!(),
                NoteEventType::MidiProgramChange => todo!(),
            }
        }

        panic!("Unable to generate a random note event after 1024 tries, this is a bug in the validator");
    }
}

impl NoteEventType {
    const ALL: &'static [NoteEventType] = &[
        NoteEventType::ClapNoteOn,
        NoteEventType::ClapNoteOff,
        NoteEventType::ClapNoteChoke,
        NoteEventType::ClapNoteExpression,
        NoteEventType::MidiNoteOn,
        NoteEventType::MidiNoteOff,
        NoteEventType::MidiChannelPressure,
        NoteEventType::MidiPolyKeyPressure,
        NoteEventType::MidiPitchBend,
        NoteEventType::MidiCc,
        NoteEventType::MidiProgramChange,
    ];
    const CLAP_EVENTS: &'static [NoteEventType] = &[
        NoteEventType::ClapNoteOn,
        NoteEventType::ClapNoteOff,
        NoteEventType::ClapNoteChoke,
        NoteEventType::ClapNoteExpression,
    ];
    const MIDI_EVENTS: &'static [NoteEventType] = &[
        NoteEventType::MidiNoteOn,
        NoteEventType::MidiNoteOff,
        NoteEventType::MidiChannelPressure,
        NoteEventType::MidiPolyKeyPressure,
        NoteEventType::MidiPitchBend,
        NoteEventType::MidiCc,
        NoteEventType::MidiProgramChange,
    ];

    /// Get a slice containing the event types supported by a plugin. Returns None if the plugin
    /// supports neither CLAP note events nor MIDI.
    pub fn supported_types(
        supports_clap_note_events: bool,
        supports_midi_events: bool,
    ) -> Option<&'static [NoteEventType]> {
        if supports_clap_note_events && supports_midi_events {
            Some(NoteEventType::ALL)
        } else if supports_clap_note_events {
            Some(NoteEventType::CLAP_EVENTS)
        } else if supports_midi_events {
            Some(NoteEventType::MIDI_EVENTS)
        } else {
            None
        }
    }
}
