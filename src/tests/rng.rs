//! Utilities for generating pseudo-random data.

use anyhow::{Context, Result};
use clap_sys::events::{
    clap_event_header, clap_event_midi, clap_event_note, clap_event_note_expression,
    clap_event_param_value, CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_MIDI, CLAP_EVENT_NOTE_CHOKE,
    CLAP_EVENT_NOTE_OFF, CLAP_EVENT_NOTE_ON, CLAP_EVENT_PARAM_VALUE, CLAP_NOTE_EXPRESSION_PRESSURE,
    CLAP_NOTE_EXPRESSION_TUNING, CLAP_NOTE_EXPRESSION_VOLUME,
};
use clap_sys::ext::note_ports::{
    CLAP_NOTE_DIALECT_CLAP, CLAP_NOTE_DIALECT_MIDI, CLAP_NOTE_DIALECT_MIDI_MPE,
};
use clap_sys::ext::params::CLAP_PARAM_IS_AUTOMATABLE;
use midi_consts::channel_event as midi;
use rand::Rng;
use rand_pcg::Pcg32;
use std::ops::RangeInclusive;

use crate::plugin::audio_thread::process::{Event, EventQueue};
use crate::plugin::ext::note_ports::NotePortConfig;
use crate::plugin::ext::params::ParamInfo;

/// Create a new pseudo-random number generator with a fixed seed.
pub fn new_prng() -> Pcg32 {
    Pcg32::new(1337, 420)
}

/// A random note and MIDI event generator that generates consistent events based on the
/// capabilities stored in a [`NotePortConfig`]
#[derive(Debug, Clone)]
pub struct NoteGenerator {
    /// The note ports to generate random events for.
    config: NotePortConfig,
    /// Only generate consistent events. This prevents things like note off events for notes that
    /// aren't playing, double note on events, and generating note expressions for notes that aren't
    /// active.
    only_consistent_events: bool,

    /// Contains the currently playing notes per-port. We'll be nice and not send overlapping notes
    /// or note-offs without a corresponding note-on.
    ///
    /// TODO: Do send overlapping notes with different note IDs if the plugin claims to support it.
    active_notes: Vec<Vec<Note>>,
    /// The CLAP note ID for the next note on event.
    next_note_id: i32,
}

/// A helper to generate random parameter automation and modulation events in a couple different
/// ways to stress test a plugin's parameter handling.
pub struct ParamFuzzer<'a> {
    config: &'a ParamInfo,
}

/// The description of an active note in the [`NoteGenerator`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Note {
    pub key: i16,
    pub channel: i16,
    pub note_id: i32,
    /// Whether the note has been choked, we can only send this event once per note.
    pub choked: bool,
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
    /// Create a new random note generator based on a plugin's note port configuration. By default
    /// these events are consistent, meaning that there are no things like note offs before a note
    /// on, duplicate note ons, or note expressions for notes that don't exist.
    pub fn new(config: NotePortConfig) -> Self {
        let num_inputs = config.inputs.len();

        NoteGenerator {
            config,
            only_consistent_events: true,

            active_notes: vec![Vec::new(); num_inputs],
            next_note_id: 0,
        }
    }

    /// Allow inconsistent events, like note off events without a corresponding note on and note
    /// expression events for notes that aren't currently playing.
    pub fn with_inconsistent_events(mut self) -> Self {
        self.only_consistent_events = false;
        self
    }

    /// Fill an event queue with random events for the next `num_samples` samples. This does not
    /// clear the event queue. If the queue was not empty, then this will do a stable sort after
    /// inserting _all_ events. If an error was returned, then the queue will not have been sorted.
    ///
    /// Returns an error if generating random events failed. This can happen if the plugin doesn't
    /// support any note event types.
    pub fn fill_event_queue<VTable>(
        &mut self,
        prng: &mut Pcg32,
        queue: &EventQueue<VTable>,
        num_samples: u32,
    ) -> Result<()> {
        // The range for the next event's timing relative to the `current_sample`. This will be
        // capped at 0, so there's a ~58% chance the next event occurs on the same time interval as
        // the previous event.
        const SAMPLE_OFFSET_RANGE: RangeInclusive<i32> = -6..=5;

        let mut events = queue.events.lock().unwrap();
        let should_sort = !events.is_empty();

        let mut current_sample = prng.gen_range(SAMPLE_OFFSET_RANGE).max(0) as u32;
        while current_sample < num_samples {
            events.push(self.generate(prng, current_sample)?);

            current_sample += prng.gen_range(SAMPLE_OFFSET_RANGE).max(0) as u32;
        }

        if should_sort {
            events.sort_by_key(|event| event.header().time);
        }

        Ok(())
    }

    /// Generate a random note event for one of the plugin's note ports depending on the port's
    /// capabilities. Returns an error if the plugin doesn't have any note ports or if the note
    /// ports don't support either MIDI or CLAP note events.
    pub fn generate(&mut self, prng: &mut Pcg32, time_offset: u32) -> Result<Event> {
        if self.config.inputs.is_empty() {
            anyhow::bail!("Cannot generate note events for a plugin with no input note ports.");
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
                .with_context(|| {
                    format!(
                        "Note input port {note_port_idx} supports neither CLAP note events nor \
                         MIDI. This is technically allowed, but few hosts will be able to \
                         interact with the plugin."
                    )
                })?;

        // We could do this in a smarter way to avoid generating impossible event types (like a note
        // off when there are no active notes), but this should work fine.
        for _ in 0..1024 {
            let event_type = prng.sample(rand::distributions::Slice::new(possible_events).unwrap());
            match event_type {
                NoteEventType::ClapNoteOn => {
                    let note = if self.only_consistent_events {
                        let key = prng.gen_range(0..128);
                        let channel = prng.gen_range(0..16);
                        let note_id = self.next_note_id;
                        let note = Note {
                            key,
                            channel,
                            note_id,
                            choked: false,
                        };
                        if self.active_notes[note_port_idx].contains(&note) {
                            continue;
                        }
                        self.active_notes[note_port_idx].push(note);
                        self.next_note_id = self.next_note_id.wrapping_add(1);

                        note
                    } else {
                        Note {
                            key: prng.gen_range(0..128),
                            channel: prng.gen_range(0..16),
                            note_id: prng.gen_range(0..100),
                            choked: false,
                        }
                    };

                    let velocity = prng.gen_range(0.0..=1.0);
                    return Ok(Event::Note(clap_event_note {
                        header: clap_event_header {
                            size: std::mem::size_of::<clap_event_note>() as u32,
                            time: time_offset,
                            space_id: CLAP_CORE_EVENT_SPACE_ID,
                            type_: CLAP_EVENT_NOTE_ON,
                            // TODO: There's a live flag here, should we also randomize this?
                            flags: 0,
                        },
                        note_id: note.note_id,
                        port_index: note_port_idx as i16,
                        channel: note.channel,
                        key: note.key,
                        velocity,
                    }));
                }
                NoteEventType::ClapNoteOff => {
                    let note = if self.only_consistent_events {
                        if self.active_notes[note_port_idx].is_empty() {
                            continue;
                        }

                        let note_idx = prng.gen_range(0..self.active_notes[note_port_idx].len());
                        self.active_notes[note_port_idx].remove(note_idx)
                    } else {
                        Note {
                            key: prng.gen_range(0..128),
                            channel: prng.gen_range(0..16),
                            note_id: prng.gen_range(0..100),
                            choked: false,
                        }
                    };

                    let velocity = prng.gen_range(0.0..=1.0);
                    return Ok(Event::Note(clap_event_note {
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
                NoteEventType::ClapNoteChoke => {
                    let note = if self.only_consistent_events {
                        if self.active_notes[note_port_idx].is_empty() {
                            continue;
                        }

                        // A note can only be choked once
                        let note_idx = prng.gen_range(0..self.active_notes[note_port_idx].len());
                        let note = &mut self.active_notes[note_port_idx][note_idx];
                        if note.choked {
                            continue;
                        }
                        note.choked = true;

                        *note
                    } else {
                        Note {
                            key: prng.gen_range(0..128),
                            channel: prng.gen_range(0..16),
                            note_id: prng.gen_range(0..100),
                            choked: false,
                        }
                    };

                    // Does a velocity make any sense here? Probably not.
                    let velocity = prng.gen_range(0.0..=1.0);
                    return Ok(Event::Note(clap_event_note {
                        header: clap_event_header {
                            size: std::mem::size_of::<clap_event_note>() as u32,
                            time: time_offset,
                            space_id: CLAP_CORE_EVENT_SPACE_ID,
                            type_: CLAP_EVENT_NOTE_CHOKE,
                            flags: 0,
                        },
                        note_id: note.note_id,
                        port_index: note_port_idx as i16,
                        channel: note.channel,
                        key: note.key,
                        velocity,
                    }));
                }
                NoteEventType::ClapNoteExpression => {
                    let note = if self.only_consistent_events {
                        if self.active_notes[note_port_idx].is_empty() {
                            continue;
                        }

                        let note_idx = prng.gen_range(0..self.active_notes[note_port_idx].len());
                        self.active_notes[note_port_idx][note_idx]
                    } else {
                        Note {
                            key: prng.gen_range(0..128),
                            channel: prng.gen_range(0..16),
                            note_id: prng.gen_range(0..100),
                            choked: false,
                        }
                    };

                    let expression_id =
                        prng.gen_range(CLAP_NOTE_EXPRESSION_VOLUME..=CLAP_NOTE_EXPRESSION_PRESSURE);
                    let value_range = match expression_id {
                        CLAP_NOTE_EXPRESSION_VOLUME => 0.0..=4.0,
                        CLAP_NOTE_EXPRESSION_TUNING => -128.0..=128.0,
                        _ => 0.0..=1.0,
                    };
                    let value = prng.gen_range(value_range);

                    return Ok(Event::NoteExpression(clap_event_note_expression {
                        header: clap_event_header {
                            size: std::mem::size_of::<clap_event_note_expression>() as u32,
                            time: time_offset,
                            space_id: CLAP_CORE_EVENT_SPACE_ID,
                            type_: CLAP_EVENT_NOTE_CHOKE,
                            flags: 0,
                        },
                        expression_id,
                        note_id: note.note_id,
                        port_index: note_port_idx as i16,
                        channel: note.channel,
                        key: note.key,
                        value,
                    }));
                }
                NoteEventType::MidiNoteOn => {
                    let note = if self.only_consistent_events {
                        let key = prng.gen_range(0..128);
                        let channel = prng.gen_range(0..16);
                        let note_id = self.next_note_id;
                        let note = Note {
                            key,
                            channel,
                            note_id,
                            choked: false,
                        };
                        if self.active_notes[note_port_idx].contains(&note) {
                            continue;
                        }
                        self.active_notes[note_port_idx].push(note);
                        self.next_note_id = self.next_note_id.wrapping_add(1);

                        note
                    } else {
                        Note {
                            key: prng.gen_range(0..128),
                            channel: prng.gen_range(0..16),
                            note_id: prng.gen_range(0..100),
                            choked: false,
                        }
                    };

                    let velocity = prng.gen_range(0.0..=1.0);
                    return Ok(Event::Midi(clap_event_midi {
                        header: clap_event_header {
                            size: std::mem::size_of::<clap_event_midi>() as u32,
                            time: time_offset,
                            space_id: CLAP_CORE_EVENT_SPACE_ID,
                            type_: CLAP_EVENT_MIDI,
                            flags: 0,
                        },
                        port_index: note_port_idx as u16,
                        data: [
                            midi::NOTE_ON | note.channel as u8,
                            note.key as u8,
                            (velocity * 127.0f32).round().clamp(0.0, 127.0) as u8,
                        ],
                    }));
                }
                NoteEventType::MidiNoteOff => {
                    let note = if self.only_consistent_events {
                        if self.active_notes[note_port_idx].is_empty() {
                            continue;
                        }

                        let note_idx = prng.gen_range(0..self.active_notes[note_port_idx].len());
                        self.active_notes[note_port_idx].remove(note_idx)
                    } else {
                        Note {
                            key: prng.gen_range(0..128),
                            channel: prng.gen_range(0..16),
                            note_id: prng.gen_range(0..100),
                            choked: false,
                        }
                    };

                    let velocity = prng.gen_range(0.0..=1.0);
                    return Ok(Event::Midi(clap_event_midi {
                        header: clap_event_header {
                            size: std::mem::size_of::<clap_event_midi>() as u32,
                            time: time_offset,
                            space_id: CLAP_CORE_EVENT_SPACE_ID,
                            type_: CLAP_EVENT_MIDI,
                            flags: 0,
                        },
                        port_index: note_port_idx as u16,
                        data: [
                            midi::NOTE_OFF | note.channel as u8,
                            note.key as u8,
                            (velocity * 127.0f32).round().clamp(0.0, 127.0) as u8,
                        ],
                    }));
                }
                NoteEventType::MidiChannelPressure => {
                    let channel = prng.gen_range(0..16);
                    let pressure = prng.gen_range(0..128);
                    return Ok(Event::Midi(clap_event_midi {
                        header: clap_event_header {
                            size: std::mem::size_of::<clap_event_midi>() as u32,
                            time: time_offset,
                            space_id: CLAP_CORE_EVENT_SPACE_ID,
                            type_: CLAP_EVENT_MIDI,
                            flags: 0,
                        },
                        port_index: note_port_idx as u16,
                        data: [midi::CHANNEL_KEY_PRESSURE | channel, pressure, 0],
                    }));
                }
                NoteEventType::MidiPolyKeyPressure => {
                    let note = if self.only_consistent_events {
                        if self.active_notes[note_port_idx].is_empty() {
                            continue;
                        }

                        let note_idx = prng.gen_range(0..self.active_notes[note_port_idx].len());
                        self.active_notes[note_port_idx][note_idx]
                    } else {
                        Note {
                            key: prng.gen_range(0..128),
                            channel: prng.gen_range(0..16),
                            note_id: prng.gen_range(0..100),
                            choked: false,
                        }
                    };

                    let pressure = prng.gen_range(0..128);
                    return Ok(Event::Midi(clap_event_midi {
                        header: clap_event_header {
                            size: std::mem::size_of::<clap_event_midi>() as u32,
                            time: time_offset,
                            space_id: CLAP_CORE_EVENT_SPACE_ID,
                            type_: CLAP_EVENT_MIDI,
                            flags: 0,
                        },
                        port_index: note_port_idx as u16,
                        data: [
                            midi::POLYPHONIC_KEY_PRESSURE | note.channel as u8,
                            note.key as u8,
                            pressure,
                        ],
                    }));
                }
                NoteEventType::MidiPitchBend => {
                    // May as well just generate the two bytes directly instead of doing fancy things
                    let channel = prng.gen_range(0..16);
                    let byte1 = prng.gen_range(0..128);
                    let byte2 = prng.gen_range(0..128);
                    return Ok(Event::Midi(clap_event_midi {
                        header: clap_event_header {
                            size: std::mem::size_of::<clap_event_midi>() as u32,
                            time: time_offset,
                            space_id: CLAP_CORE_EVENT_SPACE_ID,
                            type_: CLAP_EVENT_MIDI,
                            flags: 0,
                        },
                        port_index: note_port_idx as u16,
                        data: [midi::PITCH_BEND_CHANGE | channel, byte1, byte2],
                    }));
                }
                NoteEventType::MidiCc => {
                    let channel = prng.gen_range(0..16);
                    let cc = prng.gen_range(0..128);
                    let value = prng.gen_range(0..128);
                    return Ok(Event::Midi(clap_event_midi {
                        header: clap_event_header {
                            size: std::mem::size_of::<clap_event_midi>() as u32,
                            time: time_offset,
                            space_id: CLAP_CORE_EVENT_SPACE_ID,
                            type_: CLAP_EVENT_MIDI,
                            flags: 0,
                        },
                        port_index: note_port_idx as u16,
                        data: [midi::CONTROL_CHANGE | channel, cc, value],
                    }));
                }
                NoteEventType::MidiProgramChange => {
                    let channel = prng.gen_range(0..16);
                    let program_number = prng.gen_range(0..128);
                    return Ok(Event::Midi(clap_event_midi {
                        header: clap_event_header {
                            size: std::mem::size_of::<clap_event_midi>() as u32,
                            time: time_offset,
                            space_id: CLAP_CORE_EVENT_SPACE_ID,
                            type_: CLAP_EVENT_MIDI,
                            flags: 0,
                        },
                        port_index: note_port_idx as u16,
                        data: [midi::PROGRAM_CHANGE | channel, program_number, 0],
                    }));
                }
            }
        }

        panic!(
            "Unable to generate a random note event after 1024 tries, this is a bug in the \
             validator"
        );
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

impl<'a> ParamFuzzer<'a> {
    /// Create a new parmaeter fuzzer
    pub fn new(config: &'a ParamInfo) -> Self {
        ParamFuzzer { config }
    }

    // TODO: Modulation and per-{key,channel,port,note_id} modulation
    // TODO: Variants similar to `fill_event_queue` from `NoteGenerator`
    // TODO: A variant that snaps to the minimum or maximum value

    /// Randomize all parameters at a certain sample index using **automation**, returning an
    /// iterator yielding automation events for all parameters.
    pub fn randomize_params_at(
        &'a self,
        prng: &'a mut Pcg32,
        time_offset: u32,
    ) -> impl Iterator<Item = Event> + 'a {
        self.config
            .iter()
            .filter_map(move |(param_id, param_info)| {
                if (param_info.flags & CLAP_PARAM_IS_AUTOMATABLE) == 0 {
                    return None;
                }

                let value = if param_info.stepped() {
                    // We already confirmed that the range starts and ends in an integer when
                    // constructing the parameter info
                    prng.gen_range(param_info.range.clone()).round()
                } else {
                    prng.gen_range(param_info.range.clone())
                };

                Some(Event::ParamValue(clap_event_param_value {
                    header: clap_event_header {
                        size: std::mem::size_of::<clap_event_param_value>() as u32,
                        time: time_offset,
                        space_id: CLAP_CORE_EVENT_SPACE_ID,
                        type_: CLAP_EVENT_PARAM_VALUE,
                        flags: 0,
                    },
                    param_id: *param_id,
                    cookie: param_info.cookie,
                    note_id: -1,
                    port_index: -1,
                    channel: -1,
                    key: -1,
                    value,
                }))
            })
    }
}
