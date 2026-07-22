use crate::cli::fail_test;
use crate::cli::tracing::{Recordable, Recorder, Span, record};
use crate::plugin::util::{CHECK_POINTER, Proxy, Proxyable};
use clap_sys::events::*;
use std::fmt::Debug;
use std::sync::Mutex;

#[derive(Debug)]
pub struct InputEventQueue(Mutex<Vec<Event>>);

#[derive(Debug)]
pub struct OutputEventQueue(Mutex<Vec<Event>>);

/// An event sent to or from the plugin. This uses an enum to make the implementation simple and
/// correct at the cost of more wasteful memory usage.
#[derive(Debug, Clone)]
#[repr(C, align(8))]
pub enum Event {
    /// `CLAP_EVENT_NOTE_ON`, `CLAP_EVENT_NOTE_OFF`, `CLAP_EVENT_NOTE_CHOKE`, or `CLAP_EVENT_NOTE_END`.
    Note(clap_event_note),
    /// `CLAP_EVENT_NOTE_EXPRESSION`.
    NoteExpression(clap_event_note_expression),
    /// `CLAP_EVENT_MIDI`.
    Midi(clap_event_midi),
    /// `CLAP_EVENT_MIDI2`.
    Midi2(clap_event_midi2),
    /// `CLAP_EVENT_MIDI_SYSEX`.
    Sysex(clap_event_midi_sysex),
    /// `CLAP_EVENT_PARAM_VALUE`.
    ParamValue(clap_event_param_value),
    /// `CLAP_EVENT_PARAM_MOD`.
    ParamMod(clap_event_param_mod),
    /// `CLAP_EVENT_PARAM_GESTURE_BEGIN` or `CLAP_EVENT_PARAM_GESTURE_END`.
    ParamGesture(clap_event_param_gesture),
    /// `CLAP_EVENT_TRANSPORT`.
    Transport(clap_event_transport),
    /// An unhandled event type. This is only used when the plugin outputs an event we don't handle
    /// or recognize.
    Unknown(clap_event_header),
}

impl Proxyable for InputEventQueue {
    type Vtable = clap_input_events;

    fn init(&self) -> Self::Vtable {
        clap_input_events {
            ctx: CHECK_POINTER,
            size: Some(Self::size),
            get: Some(Self::get),
        }
    }
}

impl Proxyable for OutputEventQueue {
    type Vtable = clap_output_events;

    fn init(&self) -> Self::Vtable {
        clap_output_events {
            ctx: CHECK_POINTER,
            try_push: Some(Self::try_push),
        }
    }
}

impl InputEventQueue {
    pub fn new() -> Proxy<Self> {
        Proxy::new(Self(Mutex::new(Vec::new())))
    }

    pub fn clear(&self) {
        let mut events = self.0.lock().unwrap();
        events.clear();
    }

    pub fn last_event_time(&self) -> Option<u32> {
        let events = self.0.lock().unwrap();
        events.last().map(|event| event.header().time)
    }

    pub fn add_events(&self, extend: impl IntoIterator<Item = Event>) {
        let mut events = self.0.lock().unwrap();
        let is_empty = events.is_empty();
        events.extend(extend);
        if !is_empty {
            events.sort_by_key(|event| event.header().time);
        }
    }

    unsafe extern "C" fn size(list: *const clap_input_events) -> u32 {
        let span = Span::begin("clap_input_events::size", ());

        let state = unsafe {
            Proxy::<Self>::from_vtable(list).unwrap_or_else(|e| {
                fail_test!("clap_input_events::size: {}", e);
            })
        };

        if Proxy::vtable(&state).ctx != CHECK_POINTER {
            fail_test!("clap_input_events::size: plugin messed with the 'ctx' pointer");
        }

        let events = state.0.lock().unwrap();
        span.finish(record!(result: events.len() as u32));
        events.len() as u32
    }

    unsafe extern "C" fn get(list: *const clap_input_events, index: u32) -> *const clap_event_header {
        let span = Span::begin("clap_input_events::get", record!(index: index));

        let state = unsafe {
            Proxy::<Self>::from_vtable(list).unwrap_or_else(|e| {
                fail_test!("clap_input_events::size: {}", e);
            })
        };

        if Proxy::vtable(&state).ctx != CHECK_POINTER {
            fail_test!("clap_input_events::size: plugin messed with the 'ctx' pointer");
        }

        let events = state.0.lock().unwrap();
        match events.get(index as usize) {
            Some(event) => {
                span.finish(record!(event: event));
                event.header()
            }
            None => {
                log::warn!(
                    "The plugin tried to get an out of bounds event with index {index} ({} total events)",
                    events.len()
                );
                std::ptr::null()
            }
        }
    }
}

impl OutputEventQueue {
    pub fn new() -> Proxy<Self> {
        Proxy::new(Self(Mutex::new(Vec::new())))
    }

    pub fn clear(&self) {
        self.0.lock().unwrap().clear();
    }

    pub fn read(&self) -> Vec<Event> {
        self.0.lock().unwrap().clone()
    }

    unsafe extern "C" fn try_push(list: *const clap_output_events, event: *const clap_event_header) -> bool {
        let span = Span::begin("clap_output_events::try_push", ());
        let state = unsafe {
            Proxy::<Self>::from_vtable(list).unwrap_or_else(|e| {
                fail_test!("clap_output_events::try_push: {}", e);
            })
        };

        if Proxy::vtable(&state).ctx != CHECK_POINTER {
            fail_test!("clap_output_events::try_push: plugin messed with the 'ctx' pointer");
        }

        if event.is_null() {
            fail_test!("clap_output_events::try_push: 'event' pointer is null");
        }

        // The monotonicity of the plugin's event insertion order is checked as part of the output
        // consistency checks

        let event = unsafe { Event::from_raw(event) };
        span.finish(record!(event: event));
        state.0.lock().unwrap().push(event);
        true
    }
}

impl Event {
    /// Parse an event from a plugin-provided pointer. Returns an error if the pointer as a null pointer
    pub unsafe fn from_raw(ptr: *const clap_event_header) -> Self {
        assert!(!ptr.is_null(), "Null pointer provided for 'clap_event_header'.");

        unsafe {
            match ((*ptr).space_id, ((*ptr).type_)) {
                (
                    CLAP_CORE_EVENT_SPACE_ID,
                    CLAP_EVENT_NOTE_ON | CLAP_EVENT_NOTE_OFF | CLAP_EVENT_NOTE_CHOKE | CLAP_EVENT_NOTE_END,
                ) => Event::Note(*(ptr as *const clap_event_note)),
                (CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_NOTE_EXPRESSION) => {
                    Event::NoteExpression(*(ptr as *const clap_event_note_expression))
                }
                (CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_PARAM_VALUE) => {
                    Event::ParamValue(*(ptr as *const clap_event_param_value))
                }
                (CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_PARAM_MOD) => {
                    Event::ParamMod(*(ptr as *const clap_event_param_mod))
                }
                (CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_PARAM_GESTURE_BEGIN | CLAP_EVENT_PARAM_GESTURE_END) => {
                    Event::ParamGesture(*(ptr as *const clap_event_param_gesture))
                }
                (CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_MIDI) => Event::Midi(*(ptr as *const clap_event_midi)),
                (CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_MIDI2) => Event::Midi2(*(ptr as *const clap_event_midi2)),
                (CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_MIDI_SYSEX) => {
                    Event::Sysex(*(ptr as *const clap_event_midi_sysex))
                }
                (CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_TRANSPORT) => {
                    Event::Transport(*(ptr as *const clap_event_transport))
                }
                (_, _) => Event::Unknown(*ptr),
            }
        }
    }

    /// Get a a reference to the event's header.
    pub fn header(&self) -> &clap_event_header {
        match self {
            Event::Note(event) => &event.header,
            Event::NoteExpression(event) => &event.header,
            Event::ParamValue(event) => &event.header,
            Event::ParamMod(event) => &event.header,
            Event::ParamGesture(event) => &event.header,
            Event::Midi(event) => &event.header,
            Event::Midi2(event) => &event.header,
            Event::Sysex(event) => &event.header,
            Event::Transport(event) => &event.header,
            Event::Unknown(header) => header,
        }
    }
}

impl Recordable for Event {
    fn record(&self, record: &mut dyn Recorder) {
        record.record(
            "type",
            match (self.header().space_id, self.header().type_) {
                (CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_NOTE_ON) => "CLAP_EVENT_NOTE_ON",
                (CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_NOTE_OFF) => "CLAP_EVENT_NOTE_OFF",
                (CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_NOTE_CHOKE) => "CLAP_EVENT_NOTE_CHOKE",
                (CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_NOTE_END) => "CLAP_EVENT_NOTE_END",
                (CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_NOTE_EXPRESSION) => "CLAP_EVENT_NOTE_EXPRESSION",
                (CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_PARAM_GESTURE_BEGIN) => "CLAP_EVENT_PARAM_GESTURE_BEGIN",
                (CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_PARAM_GESTURE_END) => "CLAP_EVENT_PARAM_GESTURE_END",
                (CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_PARAM_VALUE) => "CLAP_EVENT_PARAM_VALUE",
                (CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_PARAM_MOD) => "CLAP_EVENT_PARAM_MOD",
                (CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_MIDI) => "CLAP_EVENT_MIDI",
                (CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_MIDI2) => "CLAP_EVENT_MIDI2",
                (CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_MIDI_SYSEX) => "CLAP_EVENT_MIDI_SYSEX",
                (CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_TRANSPORT) => "CLAP_EVENT_TRANSPORT",
                (_, _) => "?",
            },
        );

        record.record("space_id", self.header().space_id);
        record.record("type_id", self.header().type_);
        record.record("time", self.header().time);

        record.record("flags.is_live", self.header().flags & CLAP_EVENT_IS_LIVE != 0);
        record.record("flags.dont_record", self.header().flags & CLAP_EVENT_DONT_RECORD != 0);

        match self {
            Event::Note(event) => {
                record.record("info.note_id", event.note_id);
                record.record("info.key", event.key);
                record.record("info.port", event.port_index);
                record.record("info.channel", event.channel);
                record.record("info.velocity", event.velocity);
            }
            Event::NoteExpression(event) => {
                record.record("info.note_id", event.note_id);
                record.record("info.port_index", event.port_index);
                record.record("info.key", event.key);
                record.record("info.channel", event.channel);

                record.record(
                    "expression",
                    match event.expression_id {
                        CLAP_NOTE_EXPRESSION_VOLUME => "CLAP_NOTE_EXPRESSION_VOLUME",
                        CLAP_NOTE_EXPRESSION_PAN => "CLAP_NOTE_EXPRESSION_PAN",
                        CLAP_NOTE_EXPRESSION_TUNING => "CLAP_NOTE_EXPRESSION_TUNING",
                        CLAP_NOTE_EXPRESSION_VIBRATO => "CLAP_NOTE_EXPRESSION_VIBRATO",
                        CLAP_NOTE_EXPRESSION_BRIGHTNESS => "CLAP_NOTE_EXPRESSION_BRIGHTNESS",
                        CLAP_NOTE_EXPRESSION_PRESSURE => "CLAP_NOTE_EXPRESSION_PRESSURE",
                        CLAP_NOTE_EXPRESSION_EXPRESSION => "CLAP_NOTE_EXPRESSION_EXPRESSION",
                        _ => "?",
                    },
                );

                record.record("info.expression_id", event.expression_id);
                record.record("info.value", event.value);
            }
            Event::ParamValue(event) => {
                record.record("info.param_id", event.param_id);
                record.record("info.value", event.value);
                record.record("info.note_id", event.note_id);
                record.record("info.port_index", event.port_index);
                record.record("info.key", event.key);
                record.record("info.channel", event.channel);
            }
            Event::ParamMod(event) => {
                record.record("info.param_id", event.param_id);
                record.record("info.amount", event.amount);
                record.record("info.note_id", event.note_id);
                record.record("info.port_index", event.port_index);
                record.record("info.key", event.key);
                record.record("info.channel", event.channel);
            }
            Event::ParamGesture(event) => {
                record.record("info.param_id", event.param_id);
            }
            Event::Midi(event) => {
                record.record("info.port_index", event.port_index);
                record.record("info.raw", format_args!("{:X?}", event.data));

                if let Some(midi_event) = MidiEvent::parse(event.data) {
                    record.record("info.midi", midi_event);
                }
            }
            Event::Midi2(event) => {
                record.record("info.port_index", event.port_index);
                record.record("info.raw", format_args!("{:X?}", event.data));
            }
            Event::Sysex(event) => {
                record.record("info.port_index", event.port_index);

                if event.buffer.is_null() {
                    record.record("info.data", "<null>");
                } else {
                    record.record(
                        "info.data",
                        format_args!("{:X?}", unsafe {
                            std::slice::from_raw_parts(event.buffer, event.size as usize)
                        }),
                    );
                }
            }
            Event::Transport(event) => {
                record.record("info.transport", event);
            }
            Event::Unknown(..) => {}
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum MidiEvent {
    NoteOn { key: u8, velocity: u8, channel: u8 },
    NoteOff { key: u8, velocity: u8, channel: u8 },
    NotePressure { key: u8, pressure: u8, channel: u8 },
    ControlChange { param: u8, value: u8, channel: u8 },
    ProgramChange { program: u8, channel: u8 },
    ChannelPressure { pressure: u8, channel: u8 },
    PitchBend { value: f32, channel: u8 },
}

impl MidiEvent {
    pub fn into_bytes(self) -> [u8; 3] {
        match self {
            MidiEvent::NoteOn { key, velocity, channel } => [0x90 | channel, key & 0x7F, velocity & 0x7F],
            MidiEvent::NoteOff { key, velocity, channel } => [0x80 | channel, key & 0x7F, velocity & 0x7F],
            MidiEvent::NotePressure { key, pressure, channel } => [0xA0 | channel, key & 0x7F, pressure & 0x7F],
            MidiEvent::ControlChange { param, value, channel } => [0xB0 | channel, param & 0x7F, value & 0x7F],
            MidiEvent::ProgramChange { program, channel } => [0xC0 | channel, program & 0x7F, 0],
            MidiEvent::ChannelPressure { pressure, channel } => [0xD0 | channel, pressure & 0x7F, 0],
            MidiEvent::PitchBend { value, channel } => {
                let value = (value.clamp(-1.0, 1.0) * 8192.0) as i16 + 8192;
                [0xE0 | channel, (value & 0x7F) as u8, ((value >> 7) & 0x7F) as u8]
            }
        }
    }

    pub fn parse([a, b, c]: [u8; 3]) -> Option<Self> {
        let status = a & 0xF0;
        let channel = a & 0x0F;

        match status {
            0x80 => Some(MidiEvent::NoteOff {
                key: b,
                velocity: c,
                channel,
            }),
            0x90 if c == 0 => Some(MidiEvent::NoteOff {
                key: b,
                velocity: 0,
                channel,
            }),
            0x90 => Some(MidiEvent::NoteOn {
                key: b,
                velocity: c,
                channel,
            }),
            0xA0 => Some(MidiEvent::NotePressure {
                key: b,
                pressure: c,
                channel,
            }),
            0xB0 => Some(MidiEvent::ControlChange {
                param: b,
                value: c,
                channel,
            }),
            0xC0 => Some(MidiEvent::ProgramChange { program: b, channel }),
            0xD0 => Some(MidiEvent::ChannelPressure { pressure: b, channel }),
            0xE0 => {
                let value = ((c as u16) << 7) | (b as u16);
                let value = (value as i32 - 8192) as f32 / 8192.0;
                Some(MidiEvent::PitchBend { value, channel })
            }
            _ => None,
        }
    }
}

impl Recordable for MidiEvent {
    fn record(&self, record: &mut dyn Recorder) {
        match self {
            MidiEvent::NoteOn { key, velocity, channel } => {
                record.record("type", "Note On");
                record.record("key", *key);
                record.record("velocity", *velocity);
                record.record("channel", *channel);
            }
            MidiEvent::NoteOff { key, velocity, channel } => {
                record.record("type", "Note Off");
                record.record("key", *key);
                record.record("velocity", *velocity);
                record.record("channel", *channel);
            }
            MidiEvent::NotePressure { key, pressure, channel } => {
                record.record("type", "Aftertouch");
                record.record("key", *key);
                record.record("pressure", *pressure);
                record.record("channel", *channel);
            }
            MidiEvent::ControlChange { param, value, channel } => {
                record.record("type", "Control Change");
                record.record("control", *param);
                record.record("value", *value);
                record.record("channel", *channel);
            }
            MidiEvent::ProgramChange { program, channel } => {
                record.record("type", "Program Change");
                record.record("program", *program);
                record.record("channel", *channel);
            }
            MidiEvent::ChannelPressure { pressure, channel } => {
                record.record("type", "Channel Pressure");
                record.record("pressure", *pressure);
                record.record("channel", *channel);
            }
            MidiEvent::PitchBend { value, channel } => {
                record.record("type", "Pitch Wheel");
                record.record("value", *value);
                record.record("channel", *channel);
            }
        }
    }
}
