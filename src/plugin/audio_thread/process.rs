//! Data structures and functions surrounding audio processing.

use anyhow::Result;
use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::events::{
    clap_event_header, clap_event_midi, clap_event_note, clap_event_note_expression,
    clap_event_param_mod, clap_event_param_value, clap_event_transport, clap_input_events,
    clap_output_events, CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_MIDI, CLAP_EVENT_NOTE_CHOKE,
    CLAP_EVENT_NOTE_END, CLAP_EVENT_NOTE_EXPRESSION, CLAP_EVENT_NOTE_OFF, CLAP_EVENT_NOTE_ON,
    CLAP_EVENT_PARAM_MOD, CLAP_EVENT_PARAM_VALUE, CLAP_EVENT_TRANSPORT,
    CLAP_TRANSPORT_HAS_BEATS_TIMELINE, CLAP_TRANSPORT_HAS_SECONDS_TIMELINE,
    CLAP_TRANSPORT_HAS_TEMPO, CLAP_TRANSPORT_HAS_TIME_SIGNATURE, CLAP_TRANSPORT_IS_PLAYING,
};
use clap_sys::fixedpoint::{CLAP_BEATTIME_FACTOR, CLAP_SECTIME_FACTOR};
use clap_sys::process::clap_process;
use rand::Rng;
use rand_pcg::Pcg32;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use crate::util::check_null_ptr;

/// The input and output data for a call to `clap_plugin::process()`.
pub struct ProcessData<'a> {
    /// The input and output audio buffers.
    pub buffers: &'a mut AudioBuffers<'a>,
    /// The input events.
    pub input_events: Pin<Arc<EventQueue<clap_input_events>>>,
    /// The output events.
    pub output_events: Pin<Arc<EventQueue<clap_output_events>>>,

    config: ProcessConfig,
    /// The current transport information. This is populated when constructing this object, and the
    /// transport can be advanced `N` samples using the
    /// [`advance_transport()`][Self::advance_transport()] method.
    transport_info: clap_event_transport,
    /// The current sample position. This is used to recompute values in `transport_info`.
    sample_pos: u32,
    // TODO: Maybe do something with `steady_time`
}

/// The general context information for a process call.
#[derive(Debug, Clone, Copy)]
pub struct ProcessConfig {
    /// The current sample rate.
    pub sample_rate: f64,
    // The current tempo in beats per minute.
    pub tempo: f64,
    // The time signature's numerator.
    pub time_sig_numerator: u16,
    // The time signature's denominator.
    pub time_sig_denominator: u16,
}

/// Audio buffers for [`ProcessData`]. CLAP allows hosts to do both in-place and out-of-place
/// processing, so we'll support and test both methods.
pub enum AudioBuffers<'a> {
    /// Out-of-place processing with separate non-aliasing input and output buffers.
    OutOfPlace(OutOfPlaceAudioBuffers<'a>),
    // TODO: In-place processing, figure out a safe abstraction for this if the in-place pairs
    //       aren't symmetrical between the inputs and outputs (e.g. when it's not just
    //       input1<->output1, input2<->output2, etc.).
}

/// Audio buffers for out-of-place processing. This wrapper allocates and sets up the channel
/// pointers. To avoid an unnecessary level of abstraction where the `Vec<Vec<f32>>`s need to be
/// converted to a slice of slices, this data structure borrows the vectors directly.
//
// TODO: This only does f32 for now, we'll also want to test f64 and mixed configurations later.
pub struct OutOfPlaceAudioBuffers<'a> {
    // These are all indexed by `[port_idx][channel_idx][sample_idx]`. The inputs also need to be
    // mutable because reborrwing them from here is the only way to modify them without
    // reinitializing the pointers.
    inputs: &'a mut [Vec<Vec<f32>>],
    outputs: &'a mut [Vec<Vec<f32>>],

    // These are point to `inputs` and `outputs` because `clap_audio_buffer` needs to contain a
    // `*const *const f32`
    _input_channel_pointers: Vec<Vec<*const f32>>,
    _output_channel_pointers: Vec<Vec<*const f32>>,
    clap_inputs: Vec<clap_audio_buffer>,
    clap_outputs: Vec<clap_audio_buffer>,

    /// The number of samples for this buffer. This is consistent across all inner vectors.
    num_samples: usize,
}

// SAFETY: Sharing these pointers with other threads is safe as they refer to the borrowed input and
//         output slices. The pointers thus cannot be invalidated.
unsafe impl Send for OutOfPlaceAudioBuffers<'_> {}
unsafe impl Sync for OutOfPlaceAudioBuffers<'_> {}

/// An event queue that can be used as either an input queue or an output queue. This is always
/// allocated through a `Pin<Arc<EventQueue>>` so the pointers are stable. The `VTable` type
/// argument should be either `clap_input_events` or `clap_output_events`.
//
// NOTE: This is marked as non-exhaustive to prevent this from being constructed directly
#[derive(Debug)]
#[repr(C)]
#[non_exhaustive]
pub struct EventQueue<VTable> {
    /// The vtable for this event queue. This will be either `clap_input_events` or
    /// `clap_output_events`.
    pub vtable: VTable,
    /// The actual event queue. Since we're going for correctness over performance, this uses a very
    /// suboptimal memory layout by just using an `enum` instead of doing fancy bit packing.
    pub events: Mutex<Vec<Event>>,
}

/// An event sent to or from the plugin. This uses an enum to make the implementation simple and
/// correct at the cost of more wasteful memory usage.
#[derive(Debug)]
#[repr(C, align(8))]
pub enum Event {
    /// `CLAP_EVENT_NOTE_ON`, `CLAP_EVENT_NOTE_OFF`, `CLAP_EVENT_NOTE_CHOKE`, or `CLAP_EVENT_NOTE_END`.
    Note(clap_event_note),
    /// `CLAP_EVENT_NOTE_EXPRESSION`.
    NoteExpression(clap_event_note_expression),
    /// `CLAP_EVENT_MIDI`.
    Midi(clap_event_midi),
    /// `CLAP_EVENT_PARAM_VALUE`.
    ParamValue(clap_event_param_value),
    /// `CLAP_EVENT_PARAM_MOD`.
    ParamMod(clap_event_param_mod),
    /// An unhandled event type. This is only used when the plugin outputs an event we don't handle
    /// or recognize.
    Unknown(clap_event_header),
}

impl<'a> ProcessData<'a> {
    /// Initialize the process data using the given audio buffers. The transport information will be
    /// initialized at the start of the project, and it can be moved using the
    /// [`advance_transport()`][Self::advance_transport()] method.
    //
    // TODO: More transport info options. Missing fields, loop regions, flags, etc.
    pub fn new(buffers: &'a mut AudioBuffers<'a>, config: ProcessConfig) -> Self {
        ProcessData {
            buffers,
            input_events: EventQueue::new_input(),
            output_events: EventQueue::new_output(),

            config,
            transport_info: clap_event_transport {
                header: clap_event_header {
                    size: std::mem::size_of::<clap_event_transport>() as u32,
                    time: 0,
                    space_id: CLAP_CORE_EVENT_SPACE_ID,
                    type_: CLAP_EVENT_TRANSPORT,
                    flags: 0,
                },
                flags: CLAP_TRANSPORT_HAS_TEMPO
                    | CLAP_TRANSPORT_HAS_BEATS_TIMELINE
                    | CLAP_TRANSPORT_HAS_SECONDS_TIMELINE
                    | CLAP_TRANSPORT_HAS_TIME_SIGNATURE
                    | CLAP_TRANSPORT_IS_PLAYING,
                song_pos_beats: 0,
                song_pos_seconds: 0,
                tempo: config.tempo,
                tempo_inc: 0.0,
                // These four currently aren't used
                loop_start_beats: 0,
                loop_end_beats: 0,
                loop_start_seconds: 0,
                loop_end_seconds: 0,
                bar_start: 0,
                bar_number: 0,
                tsig_num: config.time_sig_numerator,
                tsig_denom: config.time_sig_denominator,
            },
            sample_pos: 0,
        }
    }

    /// Construct the CLAP process data, and evaluate a closure with it. The `clap_process_data`
    /// contains raw pointers to this struct's data, so the closure is there to prevent dangling
    /// pointers.
    pub fn with_clap_process_data<T, F: FnOnce(clap_process) -> T>(&mut self, f: F) -> T {
        let num_samples = self.buffers.len();
        let (inputs, outputs) = self.buffers.io_buffers();

        let process_data = clap_process {
            steady_time: self.sample_pos as i64,
            frames_count: num_samples as u32,
            transport: &self.transport_info,
            audio_inputs: if inputs.is_empty() {
                std::ptr::null()
            } else {
                inputs.as_ptr()
            },
            audio_outputs: if outputs.is_empty() {
                std::ptr::null_mut()
            } else {
                outputs.as_mut_ptr()
            },
            audio_inputs_count: inputs.len() as u32,
            audio_outputs_count: outputs.len() as u32,
            in_events: &self.input_events.vtable,
            out_events: &self.output_events.vtable,
        };

        f(process_data)
    }

    /// Get current the transport information.
    #[allow(unused)]
    pub fn transport_info(&self) -> clap_event_transport {
        self.transport_info
    }

    /// Advance the transport by a certain number of samples. Make sure to also call
    /// [`clear_events()`][Self::clear_events()].
    pub fn advance_transport(&mut self, samples: u32) {
        self.sample_pos += samples;

        self.transport_info.song_pos_beats =
            ((self.sample_pos as f64 / self.config.sample_rate / 60.0 * self.transport_info.tempo)
                * CLAP_BEATTIME_FACTOR as f64)
                .round() as i64;
        self.transport_info.song_pos_seconds = ((self.sample_pos as f64 / self.config.sample_rate)
            * CLAP_SECTIME_FACTOR as f64)
            .round() as i64;
    }

    /// Clear the event queues. Make sure to also call
    /// [`advance_transport()`][Self::advance_transport()].
    pub fn clear_events(&mut self) {
        self.input_events.events.lock().unwrap().clear();
        self.output_events.events.lock().unwrap().clear();
    }
}

impl AudioBuffers<'_> {
    /// The number of samples in the buffer.
    pub fn len(&self) -> usize {
        match self {
            AudioBuffers::OutOfPlace(buffers) => buffers.len(),
        }
    }

    /// Pointers for the inputs and the outputs. These can be used to construct the `clap_process`
    /// data.
    pub fn io_buffers(&mut self) -> (&[clap_audio_buffer], &mut [clap_audio_buffer]) {
        match self {
            AudioBuffers::OutOfPlace(buffers) => buffers.io_buffers(),
        }
    }

    /// Get a reference to the buffer's inputs.
    pub fn inputs_ref(&self) -> &[Vec<Vec<f32>>] {
        match self {
            AudioBuffers::OutOfPlace(buffers) => buffers.inputs,
        }
    }

    /// Get a reference to the buffer's outputs.
    pub fn outputs_ref(&self) -> &[Vec<Vec<f32>>] {
        match self {
            AudioBuffers::OutOfPlace(buffers) => buffers.outputs,
        }
    }

    /// Fill the input and output buffers with white noise. The values are distributed between `[-1,
    /// 1]`, and denormals are snapped to zero.
    pub fn randomize(&mut self, prng: &mut Pcg32) {
        match self {
            AudioBuffers::OutOfPlace(buffers) => buffers.randomize(prng),
        }
    }
}

impl<'a> OutOfPlaceAudioBuffers<'a> {
    /// Construct the out of place audio buffers. This allocates the channel pointers that are
    /// handed to the plugin in the process function. The function will return an error if the
    /// sample count doesn't match between all input and outputs vectors.
    pub fn new(inputs: &'a mut [Vec<Vec<f32>>], outputs: &'a mut [Vec<Vec<f32>>]) -> Result<Self> {
        // We need to make sure all inputs and outputs have the same number of channels. Since zero
        // channel ports are technically legal and it's also possible to not have any inputs we
        // can't just start with the first input.
        let mut num_samples = None;
        for channel_slices in inputs.iter().chain(outputs.iter()) {
            for channel_slice in channel_slices {
                match num_samples {
                    Some(num_samples) if channel_slice.len() != num_samples => anyhow::bail!(
                        "Inconsistent sample counts in audio buffers. Expected {}, found {}.",
                        num_samples,
                        channel_slice.len()
                    ),
                    Some(_) => (),
                    None => num_samples = Some(channel_slice.len()),
                }
            }
        }

        let input_channel_pointers: Vec<Vec<*const f32>> = inputs
            .iter()
            .map(|channel_slices| {
                channel_slices
                    .iter()
                    .map(|channel_slice| channel_slice.as_ptr())
                    .collect()
            })
            .collect();
        // These are always `*const` pointers in CLAP, even for output buffers
        let output_channel_pointers: Vec<Vec<*const f32>> = outputs
            .iter()
            .map(|channel_slices| {
                channel_slices
                    .iter()
                    .map(|channel_slice| channel_slice.as_ptr())
                    .collect()
            })
            .collect();

        let clap_inputs: Vec<clap_audio_buffer> = input_channel_pointers
            .iter()
            .map(|channel_pointers| clap_audio_buffer {
                data32: channel_pointers.as_ptr(),
                data64: std::ptr::null(),
                channel_count: channel_pointers.len() as u32,
                // TODO: Do some interesting tests with these two fields
                latency: 0,
                constant_mask: 0,
            })
            .collect();
        let clap_outputs: Vec<clap_audio_buffer> = output_channel_pointers
            .iter()
            .map(|channel_pointers| clap_audio_buffer {
                data32: channel_pointers.as_ptr(),
                data64: std::ptr::null(),
                channel_count: channel_pointers.len() as u32,
                latency: 0,
                constant_mask: 0,
            })
            .collect();

        Ok(Self {
            inputs,
            outputs,
            _input_channel_pointers: input_channel_pointers,
            _output_channel_pointers: output_channel_pointers,
            clap_inputs,
            clap_outputs,

            num_samples: num_samples.unwrap_or(0),
        })
    }

    /// The number of samples in the buffer.
    pub fn len(&self) -> usize {
        self.num_samples
    }

    /// Pointers for the inputs and the outputs. These can be used to construct the `clap_process`
    /// data.
    pub fn io_buffers(&mut self) -> (&[clap_audio_buffer], &mut [clap_audio_buffer]) {
        (&self.clap_inputs, &mut self.clap_outputs)
    }

    /// Fill the input and output buffers with white noise. The values are distributed between `[-1,
    /// 1]`, and denormals are snapped to zero.
    pub fn randomize(&mut self, prng: &mut Pcg32) {
        randomize_audio_buffers(prng, self.inputs);
        randomize_audio_buffers(prng, self.outputs);
    }
}

impl EventQueue<clap_input_events> {
    /// Construct a new event queue. This can be used as both an input and an output queue.
    pub fn new_input() -> Pin<Arc<Self>> {
        Arc::pin(EventQueue {
            vtable: clap_input_events {
                // This is not used as we can directly cast the pointer to `*const Self` because
                // this vtable is always at the start of the struct
                ctx: std::ptr::null_mut(),
                size: Self::size,
                get: Self::get,
            },
            // Using a mutex here is obviously a terrible idea in a real host, but we're not a real
            // host
            events: Mutex::new(Vec::new()),
        })
    }
}

impl EventQueue<clap_output_events> {
    /// Construct a new output event queue.
    pub fn new_output() -> Pin<Arc<Self>> {
        Arc::pin(EventQueue {
            vtable: clap_output_events {
                // This is not used as we can directly cast the pointer to `*const Self` because
                // this vtable is always at the start of the struct
                ctx: std::ptr::null_mut(),
                try_push: Self::try_push,
            },
            // Using a mutex here is obviously a terrible idea in a real host, but we're not a real
            // host
            events: Mutex::new(Vec::new()),
        })
    }
}

impl<VTable> EventQueue<VTable> {
    unsafe extern "C" fn size(list: *const clap_input_events) -> u32 {
        check_null_ptr!(0, list);
        let this = &*(list as *const Self);

        this.events.lock().unwrap().len() as u32
    }

    unsafe extern "C" fn get(
        list: *const clap_input_events,
        index: u32,
    ) -> *const clap_event_header {
        check_null_ptr!(std::ptr::null(), list);
        let this = &*(list as *const Self);

        let events = this.events.lock().unwrap();
        #[allow(clippy::significant_drop_in_scrutinee)]
        match events.get(index as usize) {
            Some(event) => event.header(),
            None => {
                log::warn!(
                    "The plugin tried to get an event with index {index} ({} total events)",
                    events.len()
                );
                std::ptr::null()
            }
        }
    }

    unsafe extern "C" fn try_push(
        list: *const clap_output_events,
        event: *const clap_event_header,
    ) -> bool {
        check_null_ptr!(false, list, event);
        let this = &*(list as *const Self);

        // The monotonicity of the plugin's event insertion order is checked as part of the output
        // consistency checks
        this.events
            .lock()
            .unwrap()
            .push(Event::from_header_ptr(event).unwrap());

        true
    }
}

impl Event {
    /// Parse an event from a plugin-provided pointer. Returns an error if the pointer as a null pointer
    pub unsafe fn from_header_ptr(ptr: *const clap_event_header) -> Result<Self> {
        if ptr.is_null() {
            anyhow::bail!("Null pointer provided for 'clap_event_header'");
        }

        match ((*ptr).space_id, ((*ptr).type_)) {
            (
                CLAP_CORE_EVENT_SPACE_ID,
                CLAP_EVENT_NOTE_ON
                | CLAP_EVENT_NOTE_OFF
                | CLAP_EVENT_NOTE_CHOKE
                | CLAP_EVENT_NOTE_END,
            ) => Ok(Event::Note(*(ptr as *const clap_event_note))),
            (CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_NOTE_EXPRESSION) => Ok(Event::NoteExpression(
                *(ptr as *const clap_event_note_expression),
            )),
            (CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_PARAM_VALUE) => {
                Ok(Event::ParamValue(*(ptr as *const clap_event_param_value)))
            }
            (CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_PARAM_MOD) => {
                Ok(Event::ParamMod(*(ptr as *const clap_event_param_mod)))
            }
            (CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_MIDI) => {
                Ok(Event::Midi(*(ptr as *const clap_event_midi)))
            }
            (_, _) => Ok(Event::Unknown(*ptr)),
        }
    }

    /// Get a a reference to the event's header.
    pub fn header(&self) -> &clap_event_header {
        match self {
            Event::Note(event) => &event.header,
            Event::NoteExpression(event) => &event.header,
            Event::ParamValue(event) => &event.header,
            Event::ParamMod(event) => &event.header,
            Event::Midi(event) => &event.header,
            Event::Unknown(header) => header,
        }
    }
}

/// Set each sample in the buffers to a random value in `[-1, 1]`. Denormals are snapped to zero.
fn randomize_audio_buffers(prng: &mut Pcg32, buffers: &mut [Vec<Vec<f32>>]) {
    for channel_slices in buffers {
        for channel_slice in channel_slices {
            for sample in channel_slice {
                *sample = prng.gen_range(-1.0..=1.0);
                if sample.is_subnormal() {
                    *sample = 0.0;
                }
            }
        }
    }
}
