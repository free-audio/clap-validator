//! Utilities for generating pseudo-random data.

use crate::plugin::ext::audio_ports::AudioPortConfig;
use crate::plugin::ext::configurable_audio_ports::{AudioPortsRequest, AudioPortsRequestInfo};
use crate::plugin::ext::note_ports::NotePortConfig;
use crate::plugin::ext::params::{Param, ParamInfo};
use crate::plugin::process::{Event, MidiEvent, TransportState};
use clap_sys::events::*;
use clap_sys::ext::ambisonic::*;
use core::f64;
use rand::seq::{IndexedRandom, IteratorRandom};
use rand::{Rng, RngExt, SeedableRng};
use std::ops::RangeInclusive;
use std::ptr::null_mut;

/// Create a new pseudo-random number generator with a fixed seed.
pub fn new_prng() -> rand::rngs::Xoshiro128PlusPlus {
    rand::rngs::Xoshiro128PlusPlus::seed_from_u64(0x1337_6767)
}

/// A random note and MIDI event generator that generates consistent events based on the
/// capabilities stored in a [`NotePortConfig`]
#[derive(Debug, Clone)]
pub struct NoteGenerator<'a> {
    /// The note ports to generate random events for.
    config: &'a NotePortConfig,

    /// The parameter info to generate random poly modulation and automation events for.
    params: Option<&'a ParamInfo>,

    /// Only generate consistent events. This prevents things like note off events for notes that
    /// aren't playing, double note on events, and generating note expressions for notes that aren't
    /// active.
    only_consistent_events: bool,

    /// Send events with wildcard values for the note ID, port index, channel, and key.
    wildcard_events: bool,

    /// Allow overlapping notes to be sent.
    overlapping_events: bool,

    /// The range for the next event's timing relative to the previous event.
    /// This will be capped to 0 when generating events
    sample_offset_range: RangeInclusive<i32>,

    /// Contains the currently playing notes. We'll be nice and not send note-offs without a corresponding note-on or
    /// overlapping note-ons if overlapping notes are not supported.
    active_notes: Vec<Note>,

    /// The CLAP note ID for the next note on event.
    next_note_id: u32,
}

/// A helper to generate random parameter automation and modulation events in a couple different
/// ways to stress test a plugin's parameter handling.
pub struct ParamFuzzer<'a> {
    /// The parameter info to generate random events for.
    pub params: &'a ParamInfo,

    /// Whether to snap generated parameter values to the parameter's minimum or maximum value.
    pub snap_to_bounds: bool,

    /// Set parameter cookies to `null` instead of the actual cookie value.
    pub no_cookies: bool,

    /// The range for the next event's timing relative to the previous event.
    /// This will be capped to 0 when generating events
    pub sample_offset_range: RangeInclusive<i32>,
}

/// A helper to generate random transport events in a couple different ways to stress test a plugin's transport handling.
pub struct TransportFuzzer {
    probability_change: f64,
}

/// The description of an active note in the [`NoteGenerator`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Note {
    pub key: u8,
    pub channel: u8,
    pub port: u32,
    pub note_id: u32,
    /// Whether the note has been choked, we can only send this event once per note.
    pub choked: bool,
    /// Whether to set the 'live' flag on events or not.
    pub live: bool,
}

struct NoteFilter {
    pub port: Option<u32>,
    pub channel: Option<u8>,
    pub key: Option<u8>,
    pub note_id: Option<u32>,
}

/// The different kinds of events we can generate. The event type chosen depends on the plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    ParamValue,
    ParamModulation,
}

impl NoteEventType {
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
    const PARAM_EVENTS: &'static [NoteEventType] = &[NoteEventType::ParamValue, NoteEventType::ParamModulation];

    /// Get a slice containing the event types supported by a plugin. Returns None if the plugin
    /// supports neither CLAP note events nor MIDI.
    pub fn supported_types(
        supports_clap_note_events: bool,
        supports_midi_events: bool,
        supports_param_events: bool,
    ) -> impl Iterator<Item = NoteEventType> {
        let clap = if supports_clap_note_events {
            Self::CLAP_EVENTS
        } else {
            &[]
        };
        let midi = if supports_midi_events { Self::MIDI_EVENTS } else { &[] };
        let param = if supports_param_events { Self::PARAM_EVENTS } else { &[] };

        clap.iter().chain(midi.iter()).chain(param.iter()).copied()
    }
}

impl Note {
    fn random(prng: &mut impl Rng) -> Self {
        Note {
            port: prng.random_range(0..10),
            key: prng.random_range(0..128),
            channel: prng.random_range(0..16),
            note_id: prng.random_range(0..100),
            live: prng.random_bool(0.1),
            choked: false,
        }
    }

    fn matches(&self, filter: &NoteFilter) -> bool {
        (filter.port.is_none() || filter.port == Some(self.port))
            && (filter.channel.is_none() || filter.channel == Some(self.channel))
            && (filter.key.is_none() || filter.key == Some(self.key))
            && (filter.note_id.is_none() || filter.note_id == Some(self.note_id))
    }
}

impl NoteFilter {
    fn from_note(note: &Note) -> Self {
        NoteFilter {
            port: Some(note.port),
            channel: Some(note.channel),
            key: Some(note.key),
            note_id: Some(note.note_id),
        }
    }

    fn random_wildcard(&self, prng: &mut impl Rng) -> Self {
        NoteFilter {
            port: if prng.random_bool(0.1) { None } else { self.port },
            channel: if prng.random_bool(0.1) { None } else { self.channel },
            key: if prng.random_bool(0.1) { None } else { self.key },
            note_id: if prng.random_bool(0.1) { None } else { self.note_id },
        }
    }

    fn raw_pckn(&self) -> (i16, i16, i16, i32) {
        (
            self.port.map(|p| p as i16).unwrap_or(-1),
            self.channel.map(|c| c as i16).unwrap_or(-1),
            self.key.map(|k| k as i16).unwrap_or(-1),
            self.note_id.map(|id| id as i32).unwrap_or(-1),
        )
    }
}

impl<'a> NoteGenerator<'a> {
    /// Create a new random note generator based on a plugin's note port configuration. By default
    /// these events are consistent, meaning that there are no things like note offs before a note
    /// on, duplicate note ons, or note expressions for notes that don't exist.
    pub fn new(config: &'a NotePortConfig) -> Self {
        NoteGenerator {
            config,
            params: None,

            only_consistent_events: true,
            overlapping_events: false,
            wildcard_events: false,

            // The range for the next event's timing relative to the `current_sample`. This will be
            // capped at 0, so there's a ~58% chance the next event occurs on the same time interval as
            // the previous event.
            sample_offset_range: -6..=5,

            active_notes: vec![],
            next_note_id: 0,
        }
    }

    /// Set the range for the next event's timing relative to the previous event. This will be
    /// clamped to 0 when generating events.
    pub fn with_sample_offset_range(mut self, range: RangeInclusive<i32>) -> Self {
        self.sample_offset_range = range;
        self
    }

    /// Set the parameter info to generate random polyphonic automation and modulation events for.
    pub fn with_params(mut self, params: &'a ParamInfo) -> Self {
        self.params = Some(params);
        self
    }

    /// Allow inconsistent events, like note off events without a corresponding note on and note
    /// expression events for notes that aren't currently playing.
    pub fn with_inconsistent_events(mut self) -> Self {
        self.only_consistent_events = false;
        self
    }

    /// Allow wildcard events, where the note ID, port index, channel, and key can be set to -1.
    pub fn with_wildcard_events(mut self) -> Self {
        self.wildcard_events = true;
        self
    }

    /// Allow overlapping notes (notes with different IDs but same key-port-channel triple).
    pub fn with_overlapping_notes(mut self) -> Self {
        self.overlapping_events = true;
        self
    }

    /// Fill an event queue with random events for the next `num_samples` samples. This does not
    /// clear the event queue. If the queue was not empty, then this will do a stable sort after
    /// inserting _all_ events.
    pub fn generate_events(&mut self, prng: &mut impl Rng, num_samples: u32) -> Vec<Event> {
        let mut events = vec![];
        let mut sample = prng.random_range(self.sample_offset_range.clone()).max(0) as u32;

        while sample < num_samples {
            let Some(event) = self.generate_event(prng, sample) else {
                break;
            };

            events.push(event);
            sample += prng.random_range(self.sample_offset_range.clone()).max(0) as u32;
        }

        events
    }

    /// Generate a random note event for one of the plugin's note ports depending on the port's
    /// capabilities. Returns an error if the plugin doesn't have any note ports or if the note
    /// ports don't support either MIDI or CLAP note events.
    pub fn generate_event(&mut self, prng: &mut impl Rng, time_offset: u32) -> Option<Event> {
        if self.config.inputs.is_empty() {
            return None;
        }

        let note_port_idx = prng.random_range(0..self.config.inputs.len());

        // We could do this in a smarter way to avoid generating impossible event types (like a note
        // off when there are no active notes), but this should work fine.
        for _ in 0..1024 {
            // We'll ignore the prefered note dialect and pick from all of the supported note dialects.
            // The plugin may get a CLAP note on and a MIDI note off if it supports both of those things
            let event_type = NoteEventType::supported_types(
                self.config.inputs[note_port_idx].supports_clap(),
                self.config.inputs[note_port_idx].supports_midi(),
                self.params.is_some(),
            )
            .choose(prng)?;

            match event_type {
                NoteEventType::ClapNoteOn => {
                    let note = if self.only_consistent_events {
                        let note = Note {
                            note_id: self.next_note_id,
                            port: note_port_idx as u32,
                            ..Note::random(prng)
                        };

                        if !self.overlapping_events
                            && self
                                .active_notes
                                .iter()
                                .any(|n| n.port == note.port && n.channel == note.channel && n.key == note.key)
                        {
                            continue;
                        }

                        self.active_notes.push(note);
                        self.next_note_id = self.next_note_id.wrapping_add(1);

                        note
                    } else {
                        Note {
                            port: note_port_idx as u32,
                            ..Note::random(prng)
                        }
                    };

                    let velocity = prng.random_range(0.0..=1.0);
                    return Some(Event::Note(clap_event_note {
                        header: clap_event_header {
                            size: std::mem::size_of::<clap_event_note>() as u32,
                            time: time_offset,
                            space_id: CLAP_CORE_EVENT_SPACE_ID,
                            type_: CLAP_EVENT_NOTE_ON,
                            flags: if note.live { CLAP_EVENT_IS_LIVE } else { 0 },
                        },
                        note_id: note.note_id as i32,
                        port_index: note.port as i16,
                        channel: note.channel as i16,
                        key: note.key as i16,
                        velocity,
                    }));
                }
                NoteEventType::ClapNoteOff => {
                    let note = if self.only_consistent_events {
                        match self.active_notes.choose(prng) {
                            Some(note) => *note,
                            _ => continue,
                        }
                    } else {
                        Note {
                            port: note_port_idx as u32,
                            ..Note::random(prng)
                        }
                    };

                    let filter = if self.wildcard_events {
                        NoteFilter::from_note(&note).random_wildcard(prng)
                    } else {
                        NoteFilter::from_note(&note)
                    };

                    self.active_notes.retain(|n| !n.matches(&filter));

                    let velocity = prng.random_range(0.0..=1.0);
                    let (port_index, channel, key, note_id) = filter.raw_pckn();
                    return Some(Event::Note(clap_event_note {
                        header: clap_event_header {
                            size: std::mem::size_of::<clap_event_note>() as u32,
                            time: time_offset,
                            space_id: CLAP_CORE_EVENT_SPACE_ID,
                            type_: CLAP_EVENT_NOTE_OFF,
                            flags: if note.live { CLAP_EVENT_IS_LIVE } else { 0 },
                        },
                        note_id,
                        port_index,
                        channel,
                        key,
                        velocity,
                    }));
                }
                NoteEventType::ClapNoteChoke => {
                    let note = if self.only_consistent_events {
                        match self.active_notes.choose(prng) {
                            Some(note) if !note.choked => *note,
                            _ => continue,
                        }
                    } else {
                        Note {
                            port: note_port_idx as u32,
                            ..Note::random(prng)
                        }
                    };

                    let filter = if self.wildcard_events {
                        NoteFilter::from_note(&note).random_wildcard(prng)
                    } else {
                        NoteFilter::from_note(&note)
                    };

                    for note in self.active_notes.iter_mut() {
                        if note.matches(&filter) {
                            note.choked = true;
                        }
                    }

                    let (port_index, channel, key, note_id) = filter.raw_pckn();
                    return Some(Event::Note(clap_event_note {
                        header: clap_event_header {
                            size: std::mem::size_of::<clap_event_note>() as u32,
                            time: time_offset,
                            space_id: CLAP_CORE_EVENT_SPACE_ID,
                            type_: CLAP_EVENT_NOTE_CHOKE,
                            flags: if note.live { CLAP_EVENT_IS_LIVE } else { 0 },
                        },
                        note_id,
                        port_index,
                        channel,
                        key,
                        velocity: f64::NAN,
                    }));
                }

                NoteEventType::ClapNoteExpression => {
                    let note = if self.only_consistent_events {
                        match self.active_notes.choose(prng) {
                            Some(note) => *note,
                            _ => continue,
                        }
                    } else {
                        Note {
                            port: note_port_idx as u32,
                            ..Note::random(prng)
                        }
                    };

                    let filter = if self.wildcard_events {
                        NoteFilter::from_note(&note).random_wildcard(prng)
                    } else {
                        NoteFilter::from_note(&note)
                    };

                    let expression_id = prng.random_range(CLAP_NOTE_EXPRESSION_VOLUME..=CLAP_NOTE_EXPRESSION_PRESSURE);
                    let value_range = match expression_id {
                        CLAP_NOTE_EXPRESSION_VOLUME => 0.0..=4.0,
                        CLAP_NOTE_EXPRESSION_TUNING => -128.0..=128.0,
                        _ => 0.0..=1.0,
                    };
                    let value = prng.random_range(value_range);
                    let (port_index, channel, key, note_id) = filter.raw_pckn();

                    return Some(Event::NoteExpression(clap_event_note_expression {
                        header: clap_event_header {
                            size: std::mem::size_of::<clap_event_note_expression>() as u32,
                            time: time_offset,
                            space_id: CLAP_CORE_EVENT_SPACE_ID,
                            type_: CLAP_EVENT_NOTE_EXPRESSION,
                            flags: if note.live { CLAP_EVENT_IS_LIVE } else { 0 },
                        },
                        expression_id,
                        note_id,
                        port_index,
                        channel,
                        key,
                        value,
                    }));
                }
                NoteEventType::MidiNoteOn => {
                    let note = if self.only_consistent_events {
                        let note = Note {
                            note_id: self.next_note_id,
                            port: note_port_idx as u32,
                            ..Note::random(prng)
                        };

                        if !self.overlapping_events
                            && self
                                .active_notes
                                .iter()
                                .any(|n| n.port == note.port && n.channel == note.channel && n.key == note.key)
                        {
                            continue;
                        }

                        self.active_notes.push(note);
                        self.next_note_id = self.next_note_id.wrapping_add(1);

                        note
                    } else {
                        Note {
                            port: note_port_idx as u32,
                            ..Note::random(prng)
                        }
                    };

                    let velocity = prng.random_range(0.0..=1.0f32);
                    let velocity = (velocity * 127.0).round().clamp(0.0, 127.0) as u8;

                    return Some(Event::Midi(clap_event_midi {
                        header: clap_event_header {
                            size: std::mem::size_of::<clap_event_midi>() as u32,
                            time: time_offset,
                            space_id: CLAP_CORE_EVENT_SPACE_ID,
                            type_: CLAP_EVENT_MIDI,
                            flags: if note.live { CLAP_EVENT_IS_LIVE } else { 0 },
                        },
                        port_index: note_port_idx as u16,
                        data: MidiEvent::NoteOn {
                            key: note.key,
                            channel: note.channel,
                            velocity,
                        }
                        .into_bytes(),
                    }));
                }
                NoteEventType::MidiNoteOff => {
                    let note = if self.only_consistent_events {
                        match self.active_notes.choose(prng) {
                            Some(note) => *note,
                            _ => continue,
                        }
                    } else {
                        Note {
                            port: note_port_idx as u32,
                            ..Note::random(prng)
                        }
                    };

                    self.active_notes
                        .retain(|n| n.port != note.port || n.channel != note.channel || n.key != note.key);

                    let velocity = prng.random_range(0.0..=1.0f32);
                    let velocity = (velocity * 127.0).round().clamp(0.0, 127.0) as u8;

                    return Some(Event::Midi(clap_event_midi {
                        header: clap_event_header {
                            size: std::mem::size_of::<clap_event_midi>() as u32,
                            time: time_offset,
                            space_id: CLAP_CORE_EVENT_SPACE_ID,
                            type_: CLAP_EVENT_MIDI,
                            flags: if note.live { CLAP_EVENT_IS_LIVE } else { 0 },
                        },
                        port_index: note_port_idx as u16,
                        data: MidiEvent::NoteOff {
                            key: note.key,
                            channel: note.channel,
                            velocity,
                        }
                        .into_bytes(),
                    }));
                }
                NoteEventType::MidiChannelPressure => {
                    let channel = prng.random_range(0..16);
                    let pressure = prng.random_range(0..128);
                    return Some(Event::Midi(clap_event_midi {
                        header: clap_event_header {
                            size: std::mem::size_of::<clap_event_midi>() as u32,
                            time: time_offset,
                            space_id: CLAP_CORE_EVENT_SPACE_ID,
                            type_: CLAP_EVENT_MIDI,
                            flags: 0,
                        },
                        port_index: note_port_idx as u16,
                        data: MidiEvent::ChannelPressure {
                            pressure,
                            channel: channel as u8,
                        }
                        .into_bytes(),
                    }));
                }
                NoteEventType::MidiPolyKeyPressure => {
                    let note = if self.only_consistent_events {
                        match self.active_notes.choose(prng) {
                            Some(note) => *note,
                            _ => continue,
                        }
                    } else {
                        Note {
                            port: note_port_idx as u32,
                            ..Note::random(prng)
                        }
                    };

                    let pressure = prng.random_range(0..128);
                    return Some(Event::Midi(clap_event_midi {
                        header: clap_event_header {
                            size: std::mem::size_of::<clap_event_midi>() as u32,
                            time: time_offset,
                            space_id: CLAP_CORE_EVENT_SPACE_ID,
                            type_: CLAP_EVENT_MIDI,
                            flags: if note.live { CLAP_EVENT_IS_LIVE } else { 0 },
                        },
                        port_index: note_port_idx as u16,
                        data: MidiEvent::NotePressure {
                            key: note.key,
                            channel: note.channel,
                            pressure,
                        }
                        .into_bytes(),
                    }));
                }
                NoteEventType::MidiPitchBend => {
                    let channel = prng.random_range(0..16);
                    let value = prng.random_range(-1.0..=1.0);
                    return Some(Event::Midi(clap_event_midi {
                        header: clap_event_header {
                            size: std::mem::size_of::<clap_event_midi>() as u32,
                            time: time_offset,
                            space_id: CLAP_CORE_EVENT_SPACE_ID,
                            type_: CLAP_EVENT_MIDI,
                            flags: 0,
                        },
                        port_index: note_port_idx as u16,
                        data: MidiEvent::PitchBend {
                            value,
                            channel: channel as u8,
                        }
                        .into_bytes(),
                    }));
                }
                NoteEventType::MidiCc => {
                    let channel = prng.random_range(0..16);
                    let param = prng.random_range(0..128);
                    let value = prng.random_range(0..128);
                    return Some(Event::Midi(clap_event_midi {
                        header: clap_event_header {
                            size: std::mem::size_of::<clap_event_midi>() as u32,
                            time: time_offset,
                            space_id: CLAP_CORE_EVENT_SPACE_ID,
                            type_: CLAP_EVENT_MIDI,
                            flags: 0,
                        },
                        port_index: note_port_idx as u16,
                        data: MidiEvent::ControlChange {
                            param,
                            value,
                            channel: channel as u8,
                        }
                        .into_bytes(),
                    }));
                }
                NoteEventType::MidiProgramChange => {
                    let channel = prng.random_range(0..16);
                    let program_number = prng.random_range(0..128);
                    return Some(Event::Midi(clap_event_midi {
                        header: clap_event_header {
                            size: std::mem::size_of::<clap_event_midi>() as u32,
                            time: time_offset,
                            space_id: CLAP_CORE_EVENT_SPACE_ID,
                            type_: CLAP_EVENT_MIDI,
                            flags: 0,
                        },
                        port_index: note_port_idx as u16,
                        data: MidiEvent::ProgramChange {
                            program: program_number,
                            channel: channel as u8,
                        }
                        .into_bytes(),
                    }));
                }
                NoteEventType::ParamValue => {
                    let Some(params) = self.params else {
                        continue;
                    };

                    let Some((param_id, param)) = params
                        .iter()
                        .filter(|(_, param)| !param.readonly() && !param.hidden() && param.poly_automatable())
                        .choose(prng)
                    else {
                        continue;
                    };

                    let note = if self.only_consistent_events {
                        match self.active_notes.choose(prng) {
                            Some(note) => *note,
                            _ => continue,
                        }
                    } else {
                        Note {
                            port: note_port_idx as u32,
                            ..Note::random(prng)
                        }
                    };

                    let filter = if self.wildcard_events {
                        NoteFilter::from_note(&note).random_wildcard(prng)
                    } else {
                        NoteFilter::from_note(&note)
                    };

                    let (port_index, channel, key, note_id) = filter.raw_pckn();
                    let value = ParamFuzzer::random_value(param, prng);

                    return Some(Event::ParamValue(clap_event_param_value {
                        header: clap_event_header {
                            size: std::mem::size_of::<clap_event_param_value>() as u32,
                            time: time_offset,
                            space_id: CLAP_CORE_EVENT_SPACE_ID,
                            type_: CLAP_EVENT_PARAM_VALUE,
                            flags: if note.live { CLAP_EVENT_IS_LIVE } else { 0 },
                        },
                        param_id: *param_id,
                        cookie: param.cookie,
                        note_id,
                        port_index,
                        channel,
                        key,
                        value,
                    }));
                }
                NoteEventType::ParamModulation => {
                    let Some(params) = self.params else {
                        continue;
                    };

                    let Some((param_id, param)) = params
                        .iter()
                        .filter(|(_, param)| !param.readonly() && !param.hidden() && param.poly_modulatable())
                        .choose(prng)
                    else {
                        continue;
                    };

                    let note = if self.only_consistent_events {
                        match self.active_notes.choose(prng) {
                            Some(note) => *note,
                            _ => continue,
                        }
                    } else {
                        Note {
                            port: note_port_idx as u32,
                            ..Note::random(prng)
                        }
                    };

                    let filter = if self.wildcard_events {
                        NoteFilter::from_note(&note).random_wildcard(prng)
                    } else {
                        NoteFilter::from_note(&note)
                    };

                    let (port_index, channel, key, note_id) = filter.raw_pckn();
                    let value = ParamFuzzer::random_value(param, prng);

                    return Some(Event::ParamValue(clap_event_param_value {
                        header: clap_event_header {
                            size: std::mem::size_of::<clap_event_param_value>() as u32,
                            time: time_offset,
                            space_id: CLAP_CORE_EVENT_SPACE_ID,
                            type_: CLAP_EVENT_PARAM_VALUE,
                            flags: if note.live { CLAP_EVENT_IS_LIVE } else { 0 },
                        },
                        param_id: *param_id,
                        cookie: param.cookie,
                        note_id,
                        port_index,
                        channel,
                        key,
                        value,
                    }));
                }
            }
        }

        panic!("Unable to generate a random note event after 1024 tries");
    }

    pub fn stop_all_voices(&mut self, time_offset: u32) -> Vec<Event> {
        let mut events = vec![];
        for note in self.active_notes.drain(..) {
            let supports_clap = self.config.inputs[note.port as usize].supports_clap();

            if supports_clap {
                events.push(Event::Note(clap_event_note {
                    header: clap_event_header {
                        size: std::mem::size_of::<clap_event_note>() as u32,
                        time: time_offset,
                        space_id: CLAP_CORE_EVENT_SPACE_ID,
                        type_: CLAP_EVENT_NOTE_OFF,
                        flags: 0,
                    },
                    note_id: note.note_id as i32,
                    port_index: note.port as i16,
                    channel: note.channel as i16,
                    key: note.key as i16,
                    velocity: 0.0,
                }));
            } else {
                events.push(Event::Midi(clap_event_midi {
                    header: clap_event_header {
                        size: std::mem::size_of::<clap_event_midi>() as u32,
                        time: time_offset,
                        space_id: CLAP_CORE_EVENT_SPACE_ID,
                        type_: CLAP_EVENT_MIDI,
                        flags: 0,
                    },
                    port_index: note.port as u16,
                    data: MidiEvent::NoteOff {
                        key: note.key,
                        channel: note.channel,
                        velocity: 0,
                    }
                    .into_bytes(),
                }));
            }
        }

        events
    }

    pub fn reset(&mut self) {
        self.next_note_id = 0;
        self.active_notes.clear();
    }
}

impl<'a> ParamFuzzer<'a> {
    /// Create a new parameter fuzzer. This ignores parameters that are readonly or hidden.
    pub fn new(params: &'a ParamInfo) -> Self {
        ParamFuzzer {
            params,
            no_cookies: false,
            snap_to_bounds: false,
            sample_offset_range: -10..=20,
        }
    }

    pub fn with_sample_offset_range(mut self, range: RangeInclusive<i32>) -> Self {
        self.sample_offset_range = range;
        self
    }

    pub fn with_no_cookies(mut self, no_cookies: bool) -> Self {
        self.no_cookies = no_cookies;
        self
    }

    pub fn snap_to_bounds(mut self, snap_to_bounds: bool) -> Self {
        self.snap_to_bounds = snap_to_bounds;
        self
    }

    /// Fill an event queue with random parameter change events for the next `num_samples` samples.
    /// This does not clear the event queue. If the queue was not empty, then this will do a stable
    /// sort after inserting _all_ events.
    ///
    /// Unlike [`ParamFuzzer::randomize_params_at`], this generates [`Event::ParamMod`] events as well as
    /// generating events at random irregular unsynchronized (between different parameters) intervals.
    pub fn generate_events(&self, prng: &mut impl Rng, num_samples: u32) -> Vec<Event> {
        let mut events = vec![];
        let mut sample = prng.random_range(self.sample_offset_range.clone()).max(0) as u32;
        while sample < num_samples {
            let Some(event) = self.generate_event(prng) else {
                break;
            };

            events.push(event);
            sample += prng.random_range(self.sample_offset_range.clone()).max(0) as u32;
        }

        events
    }

    /// Generate a single random parameter change event for one of the plugin's parameters.
    pub fn generate_event(&self, prng: &mut impl Rng) -> Option<Event> {
        let (param_id, param_info) = self
            .params
            .iter()
            .filter(|(_, info)| !info.readonly() && !info.hidden())
            .choose(prng)?;

        if !self.snap_to_bounds && param_info.modulatable() && prng.random_bool(0.5) {
            Some(Event::ParamValue(clap_event_param_value {
                header: clap_event_header {
                    size: std::mem::size_of::<clap_event_param_value>() as u32,
                    time: 0,
                    space_id: CLAP_CORE_EVENT_SPACE_ID,
                    type_: CLAP_EVENT_PARAM_VALUE,
                    flags: 0,
                },
                param_id: *param_id,
                cookie: if self.no_cookies { null_mut() } else { param_info.cookie },
                note_id: -1,
                port_index: -1,
                channel: -1,
                key: -1,
                value: ParamFuzzer::random_modulation(param_info, prng),
            }))
        } else {
            Some(Event::ParamValue(clap_event_param_value {
                header: clap_event_header {
                    size: std::mem::size_of::<clap_event_param_value>() as u32,
                    time: 0,
                    space_id: CLAP_CORE_EVENT_SPACE_ID,
                    type_: CLAP_EVENT_PARAM_VALUE,
                    flags: if param_info.automatable() {
                        0
                    } else {
                        CLAP_EVENT_IS_LIVE
                    },
                },
                param_id: *param_id,
                cookie: if self.no_cookies { null_mut() } else { param_info.cookie },
                note_id: -1,
                port_index: -1,
                channel: -1,
                key: -1,
                value: ParamFuzzer::random_value(param_info, prng),
            }))
        }
    }

    /// Randomize _all_ parameters at a certain sample index using **automation**, returning an
    /// iterator yielding automation events for all parameters.
    pub fn randomize_params_at(&'a self, prng: &'a mut impl Rng, time_offset: u32) -> impl Iterator<Item = Event> + 'a {
        self.params.iter().filter_map(move |(param_id, param_info)| {
            // We can send parameter changes for parameters that are not automatable:
            //
            // > The host can send live user changes for this parameter regardless of this flag.
            if param_info.readonly() || param_info.hidden() {
                return None;
            }

            let value = if self.snap_to_bounds {
                if prng.random_bool(0.5) {
                    *param_info.range.start()
                } else {
                    *param_info.range.end()
                }
            } else {
                ParamFuzzer::random_value(param_info, prng)
            };

            Some(Event::ParamValue(clap_event_param_value {
                header: clap_event_header {
                    size: std::mem::size_of::<clap_event_param_value>() as u32,
                    time: time_offset,
                    space_id: CLAP_CORE_EVENT_SPACE_ID,
                    type_: CLAP_EVENT_PARAM_VALUE,
                    flags: if param_info.automatable() {
                        0
                    } else {
                        CLAP_EVENT_IS_LIVE
                    },
                },
                param_id: *param_id,
                cookie: if self.no_cookies { null_mut() } else { param_info.cookie },
                note_id: -1,
                port_index: -1,
                channel: -1,
                key: -1,
                value,
            }))
        })
    }

    pub fn random_value(param: &Param, prng: &mut impl Rng) -> f64 {
        if param.stepped() {
            // We already confirmed that the range starts and ends in an integer when
            // constructing the parameter info
            prng.random_range(param.range.clone()).round()
        } else {
            prng.random_range(param.range.clone())
        }
    }

    pub fn random_modulation(param: &Param, prng: &mut impl Rng) -> f64 {
        let range = (param.range.end() - param.range.start()).abs() * 0.5;

        if param.stepped() {
            prng.random_range(-range..=range).round()
        } else {
            prng.random_range(-range..=range)
        }
    }
}

impl TransportFuzzer {
    /// Create a new transport fuzzer.
    pub fn new() -> Self {
        TransportFuzzer {
            probability_change: 0.2,
        }
    }

    /// Mutates an existing transport state.
    pub fn mutate(&mut self, prng: &mut impl Rng, transport: &mut TransportState) {
        // toggle playback state with 20% probability
        if prng.random_bool(self.probability_change) {
            transport.is_playing = !transport.is_playing;
        }

        // toggle recording state with 20% probability
        if prng.random_bool(self.probability_change) {
            transport.is_recording = !transport.is_recording;
        }

        // change time signature with 20% probability
        if prng.random_bool(self.probability_change) {
            if prng.random_bool(0.5) {
                transport.time_signature = None;
            } else {
                transport.time_signature = Some((prng.random_range(1..=16), prng.random_range(1..=4)));
            }
        }

        // change tempo (instanteous) with 20% probability
        if prng.random_bool(self.probability_change) {
            if prng.random_bool(0.5) {
                transport.tempo = None;
            } else {
                transport.tempo = Some((prng.random_range(40.0..=480.0), 0.0));
            }
        }

        // change tempo (ramp) with 40% probability
        if let Some((tempo, ramp)) = &mut transport.tempo
            && prng.random_bool(self.probability_change)
        {
            // safeguard to prevent extremely low tempos
            if *tempo < 20.0 {
                *tempo = 20.0;
                *ramp = prng.random_range(0.0..=0.01);
            }

            *ramp = prng.random_range(-0.01..=0.01);
        }

        // seek to a new position with 10% probability
        if prng.random_bool(self.probability_change) {
            if prng.random_bool(0.5) {
                transport.position_seconds = None;
            } else {
                transport.position_seconds = Some(prng.random_range(0.0..=60.0));
            }

            if prng.random_bool(0.5) {
                transport.position_beats = None;
            } else {
                transport.position_beats = Some(prng.random_range(0.0..=240.0));
            }

            if prng.random_bool(0.5) {
                transport.sample_pos = None;
            } else {
                // we can only seek forward
                transport.sample_pos = Some(transport.sample_pos.unwrap_or(0) + prng.random_range(0..=100_000) as u64);
            }
        }

        if transport.tempo.is_none() {
            transport.position_beats = None;
        }

        if !transport.is_playing
            && let Some((_, ramp)) = &mut transport.tempo
        {
            *ramp = 0.0;
        }
    }
}

pub fn random_layout_requests(config: &AudioPortConfig, prng: &mut impl Rng) -> Vec<AudioPortsRequest<'static>> {
    fn random_request_info(prng: &mut impl Rng) -> AudioPortsRequestInfo<'static> {
        match prng.random_range(0..=4) {
            0 => AudioPortsRequestInfo::Mono,
            1 => AudioPortsRequestInfo::Stereo,
            2 => AudioPortsRequestInfo::Untyped {
                channel_count: prng.random_range(1..=16),
            },
            3 => {
                const AMBISONIC_ACN_SN3D: clap_ambisonic_config = clap_ambisonic_config {
                    ordering: CLAP_AMBISONIC_ORDERING_ACN,
                    normalization: CLAP_AMBISONIC_NORMALIZATION_SN3D,
                };

                const AMBISONIC_FUMA_MAXN: clap_ambisonic_config = clap_ambisonic_config {
                    ordering: CLAP_AMBISONIC_ORDERING_FUMA,
                    normalization: CLAP_AMBISONIC_NORMALIZATION_MAXN,
                };

                let channel_count = prng.random_range(1..=4u32).pow(2);
                let is_acn_sn3d = prng.random_bool(0.5);

                AudioPortsRequestInfo::Ambisonic {
                    channel_count,
                    config: if is_acn_sn3d {
                        &AMBISONIC_ACN_SN3D
                    } else {
                        &AMBISONIC_FUMA_MAXN
                    },
                }
            }
            _ => {
                const SURROUND_MAPS: &[&[u8]] = &[
                    &[2],                 // Mono;   FC
                    &[0, 1],              // Stereo; FL FR
                    &[0, 2, 1],           // 3.0;    FL FC FR
                    &[0, 2, 1, 3],        // 3.1;    FL FC FR LFE
                    &[0, 2, 1, 8],        // 4.0;    FL FC FR BC
                    &[0, 2, 1, 8, 3],     // 4.1;    FL FC FR BC LFE
                    &[0, 2, 1, 9, 10],    // 5.0;    FL FC FR SL SR
                    &[0, 2, 1, 9, 10, 3], // 5.1;    FL FC FR SL SR LFE
                    &[0, 1, 2],           // 3.0;    FL FR FC
                    &[0, 1, 2, 3],        // 3.1;    FL FR FC LFE
                    &[0, 1, 2, 8],        // 4.0;    FL FR FC BC
                    &[0, 1, 2, 3, 8],     // 4.1;    FL FR FC LFE BC
                    &[0, 1, 2, 9, 10],    // 5.0;    FL FR FC SL SR
                    &[0, 1, 2, 3, 9, 10], // 5.1;    FL FR FC LFE SL SR
                ];

                AudioPortsRequestInfo::Surround {
                    channel_map: SURROUND_MAPS.choose(prng).unwrap(),
                }
            }
        }
    }

    let mut requests = vec![];

    for index in 0..config.inputs.len() {
        if prng.random_bool(0.1) {
            // skip request for some inputs
            continue;
        }

        requests.push(AudioPortsRequest {
            is_input: true,
            port_index: index as u32,
            request_info: random_request_info(prng),
        });
    }

    for index in 0..config.outputs.len() {
        if prng.random_bool(0.1) {
            // skip request for some outputs
            continue;
        }

        requests.push(AudioPortsRequest {
            is_input: false,
            port_index: index as u32,
            request_info: random_request_info(prng),
        });
    }

    // throw in random (maybe invalid) requests
    while prng.random_bool(0.2) {
        let is_input = prng.random_bool(0.5);
        let port_index = if is_input {
            prng.random_range(config.inputs.len() as u32..=config.inputs.len() as u32 + 10)
        } else {
            prng.random_range(config.outputs.len() as u32..=config.outputs.len() as u32 + 10)
        };

        requests.push(AudioPortsRequest {
            is_input,
            port_index,
            request_info: random_request_info(prng),
        });
    }

    requests
}
