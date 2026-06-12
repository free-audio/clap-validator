//! Abstractions for single CLAP plugin instances for audio thread interactions.

use super::{Plugin, PluginStatus};
use crate::cli::tracing::{Recordable, Recorder, Span, record};
use crate::plugin::ext::Extension;
use crate::plugin::instance::{CallbackEvent, PluginShared};
use crate::plugin::process::{InputEventQueue, OutputEventQueue};
use crate::plugin::util::{Proxy, clap_call};
use anyhow::Result;
use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::events::clap_event_transport;
use clap_sys::plugin::clap_plugin;
use clap_sys::process::*;
use std::fmt::Debug;
use std::marker::PhantomData;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::Sender;

pub type MainThreadTask = Box<dyn FnOnce(&Plugin) -> Result<()> + Send>;

/// An audio thread equivalent to [`Plugin`]. This version only allows audio thread functions to be
/// called. It can be constructed using [`Plugin::on_audio_thread()`].
pub struct PluginAudioThread<'a> {
    /// Information about this plugin instance stored on the host. This keeps track of things like
    /// audio thread IDs, whether the plugin has pending callbacks, and what state it is in.
    shared: Proxy<PluginShared>,

    /// A channel to send tasks to the main thread.
    /// Allows for ergonomic access to the main thread OR executing tasks on the main thread in parallel with the audio thread.
    sender: Sender<MainThreadTask>,

    _plugin_marker: PhantomData<&'a Plugin<'a>>,

    /// To honor CLAP's thread safety guidelines, the thread this object was created from is
    /// designated the 'audio thread', and this object cannot be shared with other threads.
    _send_sync_marker: PhantomData<*const ()>,
}

/// The equivalent of `clap_process_status`, minus the `CLAP_PROCESS_ERROR` value as this is already
/// treated as an error by `PluginAudioThread::process()`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProcessStatus {
    Continue,
    ContinueIfNotQuiet,
    Tail,
    Sleep,
}

#[derive(Debug)]
pub struct ProcessInfo<'a> {
    pub frames_count: u32,
    pub steady_time: Option<u64>,
    pub transport: Option<&'a clap_event_transport>,
    pub audio_inputs: &'a [clap_audio_buffer],
    pub audio_outputs: &'a mut [clap_audio_buffer],
    pub input_events: &'a Proxy<InputEventQueue>,
    pub output_events: &'a Proxy<OutputEventQueue>,
}

impl Drop for PluginAudioThread<'_> {
    fn drop(&mut self) {
        self.shared.audio_thread_id.store(None);
    }
}

impl<'a> PluginAudioThread<'a> {
    pub(super) fn new(shared: Proxy<PluginShared>, sender: Sender<MainThreadTask>) -> PluginAudioThread<'a> {
        shared.audio_thread_id.store(Some(std::thread::current().id()));

        PluginAudioThread {
            sender,
            shared,
            _plugin_marker: PhantomData,
            _send_sync_marker: PhantomData,
        }
    }

    /// Get the raw pointer to the `clap_plugin` instance.
    pub fn as_ptr(&self) -> *const clap_plugin {
        self.shared.clap_plugin
    }

    /// Get the plugin's current initialization status.
    pub fn status(&self) -> PluginStatus {
        self.shared.status()
    }

    /// Get a reference to the plugin's shared state.
    pub fn shared(&self) -> &PluginShared {
        &self.shared
    }

    /// Get the _audio thread_ extension abstraction for the extension `T`, if the plugin supports
    /// this extension. Returns `None` if it does not.
    pub fn get_extension<T: Extension<Plugin = &'a Self>>(&'a self) -> Option<T> {
        unsafe { self.shared.raw_extension::<T>().map(|ptr| T::new(self, ptr)) }
    }

    /// Dispatch a task to be executed on the main thread. This is a blocking call that will wait
    /// for the task to complete and return its result.
    pub fn on_main_thread<F: FnOnce(&Plugin) -> T + Send, T: Send>(&self, callback: F) -> T {
        let (sender, recv) = std::sync::mpsc::sync_channel(0);

        #[allow(clippy::type_complexity)]
        let callback: Box<dyn FnOnce(&Plugin) -> Result<()> + Send> = Box::new(move |plugin| {
            let result = catch_unwind(AssertUnwindSafe(|| callback(plugin)));
            sender.send(result).unwrap();
            Ok(())
        });

        self.sender
            .send(unsafe {
                // SAFETY: we just erase the lifetime here, as we guarantee that the callback is valid until Receiver is dropped, at that point the callback has been dropped already.
                std::mem::transmute::<
                    Box<dyn FnOnce(&Plugin) -> Result<()> + Send>,
                    Box<dyn FnOnce(&Plugin) -> Result<()> + Send + 'static>,
                >(callback)
            })
            .unwrap();

        match recv.recv().unwrap() {
            Ok(value) => value,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }

    /// Same as [`Self::on_main_thread`], but does not wait for the result and does not block.
    #[allow(unused)]
    pub fn send_main_thread<F: FnOnce(&Plugin) -> Result<()> + Send + 'static>(&self, callback: F) {
        self.sender.send(Box::new(callback)).unwrap();
    }

    /// Process pending callbacks.
    pub fn poll_callback_with(&self, mut f: impl FnMut(&Plugin, CallbackEvent) -> Result<()> + Send) -> Result<()> {
        if self.shared.requested_callback.load() {
            self.on_main_thread(move |plugin| plugin.poll_callback(|event| f(plugin, event)))
        } else {
            Ok(())
        }
    }

    /// Process pending callbacks, ignoring the callback events. Does not block.
    pub fn poll_callback(&self) {
        if self.shared.requested_callback.load() {
            self.send_main_thread(move |plugin| {
                plugin.poll_callback_unchecked();
                Ok(())
            });
        }
    }

    /// Prepare for audio processing. Returns an error if the plugin returned `false`. See
    /// [plugin.h](https://github.com/free-audio/clap/blob/main/include/clap/plugin.h) for the
    /// preconditions.
    pub fn start_processing(&self) -> Result<()> {
        self.status().assert_is(PluginStatus::Activated);

        let span = Span::begin("clap_plugin::start_processing", ());
        let result = unsafe {
            clap_call! { self.as_ptr()=>start_processing(self.as_ptr()) }
        };

        span.finish(record!(result: result));

        if result {
            self.shared.set_status(PluginStatus::Processing);
            Ok(())
        } else {
            anyhow::bail!("'clap_plugin::start_processing()' returned false.")
        }
    }

    /// Process audio. If the plugin returned either `CLAP_PROCESS_ERROR` or an unknown process
    /// status code, then this will return an error. See
    /// [plugin.h](https://github.com/free-audio/clap/blob/main/include/clap/plugin.h) for the
    /// preconditions.
    pub fn process(&self, process: ProcessInfo) -> Result<ProcessStatus> {
        self.status().assert_is(PluginStatus::Processing);

        self.shared.is_currently_in_process_call.store(true);

        let span = Span::begin("clap_plugin::process", &process);

        let result = unsafe {
            clap_call! { self.as_ptr()=>process(self.as_ptr(), &clap_process {
                frames_count: process.frames_count,
                steady_time: process.steady_time.map(|t| t as i64).unwrap_or(-1),
                transport: process.transport.map_or(std::ptr::null(), |t| t as *const clap_event_transport),
                audio_inputs: process.audio_inputs.as_ptr(),
                audio_outputs: process.audio_outputs.as_mut_ptr(),
                audio_inputs_count: process.audio_inputs.len() as u32,
                audio_outputs_count: process.audio_outputs.len() as u32,
                in_events: Proxy::vtable(process.input_events),
                out_events: Proxy::vtable(process.output_events),
            }) }
        };

        span.finish(record!(
            result: match result {
                CLAP_PROCESS_ERROR => "CLAP_PROCESS_ERROR",
                CLAP_PROCESS_CONTINUE => "CLAP_PROCESS_CONTINUE",
                CLAP_PROCESS_CONTINUE_IF_NOT_QUIET => "CLAP_PROCESS_CONTINUE_IF_NOT_QUIET",
                CLAP_PROCESS_TAIL => "CLAP_PROCESS_TAIL",
                CLAP_PROCESS_SLEEP => "CLAP_PROCESS_SLEEP",
                _ => "?",
            }
        ));

        self.shared.is_currently_in_process_call.store(false);

        Ok(match result {
            CLAP_PROCESS_CONTINUE => ProcessStatus::Continue,
            CLAP_PROCESS_CONTINUE_IF_NOT_QUIET => ProcessStatus::ContinueIfNotQuiet,
            CLAP_PROCESS_TAIL => ProcessStatus::Tail,
            CLAP_PROCESS_SLEEP => ProcessStatus::Sleep,
            CLAP_PROCESS_ERROR => {
                anyhow::bail!("The plugin returned 'CLAP_PROCESS_ERROR' from 'clap_plugin::process()'.")
            }
            result => anyhow::bail!(
                "The plugin returned an unknown 'clap_process_status' value {result} from 'clap_plugin::process()'."
            ),
        })
    }

    /// Reset the internal state of the plugin.
    pub fn reset(&self) {
        self.status().assert_active();

        unsafe {
            let _span = Span::begin("clap_plugin::reset", ());
            clap_call! { self.as_ptr()=>reset(self.as_ptr()) }
        };
    }

    /// Stop processing audio. See
    /// [plugin.h](https://github.com/free-audio/clap/blob/main/include/clap/plugin.h) for the
    /// preconditions.
    pub fn stop_processing(&self) {
        self.status().assert_is(PluginStatus::Processing);

        unsafe {
            let _span = Span::begin("clap_plugin::stop_processing", ());
            clap_call! { self.as_ptr()=>stop_processing(self.as_ptr()) }
        };

        self.shared.set_status(PluginStatus::Activated);
    }
}

impl Recordable for ProcessInfo<'_> {
    fn record(&self, record: &mut dyn Recorder) {
        record.record("frames_count", self.frames_count);
        record.record("steady_time", self.steady_time.map(|t| t as i64).unwrap_or(-1));

        if let Some(transport) = self.transport {
            record.record("transport", transport);
        }

        for i in 0..self.audio_inputs.len() {
            record.record(
                &format!("audio_input.{i}.channel_count"),
                self.audio_inputs[i].channel_count,
            );
            record.record(
                &format!("audio_input.{i}.data32"),
                format_args!("{:p}", self.audio_inputs[i].data32),
            );
            record.record(
                &format!("audio_input.{i}.data64"),
                format_args!("{:p}", self.audio_inputs[i].data64),
            );
            record.record(
                &format!("audio_input.{i}.constant_mask"),
                format_args!("0b{:b}", self.audio_inputs[i].constant_mask),
            );
            record.record(&format!("audio_input.{i}.latency"), self.audio_inputs[i].latency);
        }

        for i in 0..self.audio_outputs.len() {
            record.record(
                &format!("audio_output.{i}.channel_count"),
                self.audio_outputs[i].channel_count,
            );
            record.record(
                &format!("audio_output.{i}.data32"),
                format_args!("{:p}", self.audio_outputs[i].data32),
            );
            record.record(
                &format!("audio_output.{i}.data64"),
                format_args!("{:p}", self.audio_outputs[i].data64),
            );
            record.record(&format!("audio_output.{i}.latency"), self.audio_outputs[i].latency);
        }
    }
}

impl Debug for ProcessStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProcessStatus::Continue => write!(f, "CLAP_PROCESS_CONTINUE"),
            ProcessStatus::ContinueIfNotQuiet => write!(f, "CLAP_PROCESS_CONTINUE_IF_NOT_QUIET"),
            ProcessStatus::Tail => write!(f, "CLAP_PROCESS_TAIL"),
            ProcessStatus::Sleep => write!(f, "CLAP_PROCESS_SLEEP"),
        }
    }
}
