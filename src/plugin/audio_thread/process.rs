//! Data structures and functions surrounding audio processing.

use std::ffi::c_void;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::events::{
    clap_event_header, clap_event_transport, clap_input_events, clap_output_events,
    CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_TRANSPORT, CLAP_TRANSPORT_HAS_BEATS_TIMELINE,
    CLAP_TRANSPORT_HAS_SECONDS_TIMELINE, CLAP_TRANSPORT_HAS_TEMPO,
    CLAP_TRANSPORT_HAS_TIME_SIGNATURE, CLAP_TRANSPORT_IS_PLAYING,
};
use clap_sys::fixedpoint::{CLAP_BEATTIME_FACTOR, CLAP_SECTIME_FACTOR};
use clap_sys::process::clap_process;

use crate::util::check_null_ptr;

/// The input and output data for a call to `clap_plugin::process()`.
pub struct ProcessData<'a> {
    /// The input and output audio buffers.
    pub buffers: AudioBuffers<'a>,
    /// The input events.
    pub input_events: Pin<Arc<EventQueue>>,
    /// The output events.
    pub output_events: Pin<Arc<EventQueue>>,
    /// The current transport information. This is populated when constructing this object, and the
    /// transport can be advanced `N` samples using the
    /// [`advance_transport()`][Self::advance_transport()] method.
    transport_info: clap_event_transport,
    /// The current sample position. This is used to recompute values in `transport_info`.
    sample_pos: u32,
    /// The current sample rate.
    sample_rate: f64,
    // TODO: Events
    // TODO: Maybe do something with `steady_time`
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
    // These are all indexed by `[port_idx][channel_idx][sample_idx]`
    inputs: &'a [Vec<Vec<f32>>],
    outputs: &'a mut [Vec<Vec<f32>>],

    // These are point to `inputs` and `outputs` because `clap_audio_buffer` needs to contain a
    // `*const *const f32`
    input_channel_pointers: Vec<Vec<*const f32>>,
    output_channel_pointers: Vec<Vec<*const f32>>,
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
/// allocated through a `Pin<Arc<EventQueue>>` so the pointers are stable.
//
// NOTE: This is marked as non-exhaustive to prevent this from being constructed directly
#[non_exhaustive]
pub struct EventQueue {
    /// The vtable for the read-only variant of the event queue.
    pub in_events: clap_input_events,
    /// The vtable for the write-only variant of the event queue.
    pub out_events: clap_output_events,
    /// The actual event queue. Since we're going for correctness over performance, this uses a very
    /// suboptimal memory layout by just using an `enum` instead of doing fancy bit packing.
    pub events: Mutex<Vec<Event>>,
}

/// An event sent to or from the plugin. This uses an enum to make the implementation simple and
/// correct at the cost of more wasteful memory usage.
#[repr(align(8))]
pub enum Event {
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
    pub fn new(
        buffers: AudioBuffers<'a>,
        sample_rate: f64,
        tempo: f64,
        time_sig_numerator: u16,
        time_sig_denominator: u16,
    ) -> Self {
        ProcessData {
            buffers,
            input_events: EventQueue::new(),
            output_events: EventQueue::new(),
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
                tempo,
                tempo_inc: 0.0,
                // These four currently aren't used
                loop_start_beats: 0,
                loop_end_beats: 0,
                loop_start_seconds: 0,
                loop_end_seconds: 0,
                bar_start: 0,
                bar_number: 0,
                tsig_num: time_sig_numerator,
                tsig_denom: time_sig_denominator,
            },
            sample_pos: 0,
            sample_rate,
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
            audio_inputs: inputs.as_ptr(),
            audio_outputs: outputs.as_mut_ptr(),
            audio_inputs_count: inputs.len() as u32,
            audio_outputs_count: outputs.len() as u32,
            in_events: &self.input_events.in_events,
            out_events: &self.output_events.out_events,
        };

        f(process_data)
    }

    /// Get current the transport information.
    pub fn transport_info(&self) -> clap_event_transport {
        self.transport_info
    }

    /// Advance the transport by a certain number of samples
    pub fn advance_transport(&mut self, samples: u32) {
        self.sample_pos += samples;

        self.transport_info.song_pos_beats = ((self.sample_pos as f64 / self.sample_rate / 60.0
            * self.transport_info.tempo)
            * CLAP_BEATTIME_FACTOR as f64)
            .round() as i64;
        self.transport_info.song_pos_seconds = ((self.sample_pos as f64 / self.sample_rate)
            * CLAP_SECTIME_FACTOR as f64)
            .round() as i64;
    }
}

impl AudioBuffers<'_> {
    /// The number of samples in the buffer.
    pub fn len(&self) -> usize {
        match &self {
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
}

impl<'a> OutOfPlaceAudioBuffers<'a> {
    /// Construct the out of place audio buffers. This allocates the channel pointers that are
    /// handed to the plugin in the process function. The function will return an error if the
    /// sample count doesn't match between all input and outputs vectors.
    pub fn new(inputs: &'a [Vec<Vec<f32>>], outputs: &'a mut [Vec<Vec<f32>>]) -> Result<Self> {
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
            input_channel_pointers,
            output_channel_pointers,
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
}

impl EventQueue {
    /// Construct a new event queue. This can be used as both an input and an output queue.
    pub fn new() -> Pin<Arc<Self>> {
        let mut queue = Arc::new(EventQueue {
            in_events: clap_input_events {
                // This field is set later in this function since we can't do a straight up pointer
                // case here because we have two vtables
                ctx: std::ptr::null_mut(),
                size: Self::size,
                get: Self::get,
            },
            out_events: clap_output_events {
                // Same here
                ctx: std::ptr::null_mut(),
                try_push: Self::try_push,
            },
            // Using a mutex here is obviously a terrible idea in a real host, but we're not a real
            // host
            events: Mutex::new(Vec::new()),
        });

        // Fun
        {
            let queue_ptr = Arc::as_ptr(&queue);
            let queue = Arc::get_mut(&mut queue).unwrap();
            queue.in_events.ctx = queue_ptr as *mut c_void;
            queue.out_events.ctx = queue_ptr as *mut c_void;
        }

        Pin::new(queue)
    }

    unsafe extern "C" fn size(list: *const clap_input_events) -> u32 {
        check_null_ptr!(0, list, (*list).ctx);
        let this = &*((*list).ctx as *const Self);

        this.events.lock().unwrap().len() as u32
    }

    unsafe extern "C" fn get(
        list: *const clap_input_events,
        index: u32,
    ) -> *const clap_event_header {
        check_null_ptr!(std::ptr::null(), list, (*list).ctx);
        let this = &*((*list).ctx as *const Self);

        let events = this.events.lock().unwrap();
        match events.get(index as usize) {
            Some(event) => event.header_ptr(),
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
        check_null_ptr!(false, list, (*list).ctx, event);
        let this = &*((*list).ctx as *const Self);

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

        // TODO: Implement some events for things we'll send the plugin
        Ok(Event::Unknown(*ptr))
    }

    /// Get a pointer to the event's header
    pub fn header_ptr(&self) -> *const clap_event_header {
        match &self {
            Event::Unknown(header) => header,
        }
    }
}
