//! Data structures and functions surrounding audio processing.
use crate::plugin::instance::{PluginAudioThread, PluginStatus, ProcessInfo, ProcessStatus};
use crate::plugin::util::Proxy;
use anyhow::Result;

mod buffer;
mod events;
mod transport;

pub use buffer::*;
use either::Either;
pub use events::*;
pub use transport::*;

pub struct ProcessScope<'a> {
    plugin: &'a PluginAudioThread<'a>,
    buffer: &'a mut AudioBuffers,

    events_input: Proxy<InputEventQueue>,
    events_output: Proxy<OutputEventQueue>,

    transport: TransportState,
    sample_rate: f64,
    min_buffer_size: u32,

    check_denormals: bool,
    check_outputs: Vec<bool>,
}

impl<'a> ProcessScope<'a> {
    pub fn new(plugin: &'a PluginAudioThread, buffer: &'a mut AudioBuffers) -> Result<Self> {
        Self::with_config(plugin, buffer, 44100.0, 1)
    }

    pub fn with_config(
        plugin: &'a PluginAudioThread,
        buffer: &'a mut AudioBuffers,
        sample_rate: f64,
        min_buffer_size: u32,
    ) -> Result<Self> {
        plugin.status().assert_is(PluginStatus::Deactivated);

        Ok(ProcessScope {
            check_denormals: true,
            check_outputs: vec![true; buffer.num_outputs()],

            plugin,
            buffer,
            events_input: InputEventQueue::new(),
            events_output: OutputEventQueue::new(),
            transport: TransportState::dummy(),
            sample_rate,
            min_buffer_size,
        })
    }

    pub fn set_allow_denormals(&mut self, allow: bool) {
        self.check_denormals = !allow;
    }

    pub fn set_output_active(&mut self, index: u32, active: bool) {
        if let Some(mask) = self.check_outputs.get_mut(index as usize) {
            *mask = active;
        }
    }

    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    pub fn max_block_size(&self) -> u32 {
        self.buffer.samples()
    }

    pub fn wants_restart(&self) -> bool {
        self.plugin.shared().requested_restart.load()
    }

    pub fn add_events(&mut self, events: impl IntoIterator<Item = Event>) {
        self.events_input.add_events(events);
    }

    #[allow(unused)]
    pub fn read_events(&self) -> Vec<Event> {
        self.events_output.read()
    }

    pub fn transport(&mut self) -> &mut TransportState {
        &mut self.transport
    }

    pub fn audio_buffers(&mut self) -> &mut AudioBuffers {
        self.buffer
    }

    pub fn reset(&mut self) {
        if self.plugin.status() >= PluginStatus::Activated {
            self.plugin.reset();
        }
    }

    pub fn run(&mut self) -> Result<ProcessStatus> {
        self.run_with(self.buffer.samples())
    }

    pub fn run_with(&mut self, block_size: u32) -> Result<ProcessStatus> {
        assert!(block_size > 0 && block_size <= self.buffer.samples());

        self.activate()?;

        // check that we dont overfill the input event queue
        assert!(
            self.events_input.last_event_time().is_none_or(|t| t < block_size),
            "The input event queue contains events beyond the current processing block size"
        );

        // prepare output event queue for processing
        self.events_output.clear();

        // prepare output audio buffers for processing
        // this is used to detect uninitialized output buffers
        for buffer in self.buffer.iter_mut() {
            if buffer.port().input().is_none() {
                buffer.fill(CHECK_NAN_F32, CHECK_NAN_F64);
            }
        }

        // save original buffers for consistency check
        let original_buffers = self.buffer[..].to_owned();

        // run processing
        let status = self.buffer.process(|inputs, outputs| {
            let transport = self.transport.as_clap_transport(0);
            self.plugin.process(ProcessInfo {
                frames_count: block_size,
                steady_time: self.transport.sample_pos,
                audio_inputs: inputs,
                audio_outputs: outputs,
                input_events: &self.events_input,
                output_events: &self.events_output,
                transport: (!self.transport.is_freerun).then_some(&transport),
            })
        })?;

        // clear input event queue and advance transport
        self.events_input.clear();
        self.transport.advance(block_size as i64, self.sample_rate());

        // check output audio buffers for NaNs or infinities
        check_process_call_consistency(
            &self.buffer[..],
            &original_buffers,
            &self.events_output.read(),
            block_size,
            self.check_denormals,
            &self.check_outputs,
        )?;

        Ok(status)
    }

    /// Activate/start processing if needed.
    ///
    /// The state will be [`PluginStatus::Processing`] if successful.
    pub fn activate(&mut self) -> Result<()> {
        if self.plugin.shared().requested_restart.load() {
            log::debug!("Plugin has requested a restart");
            self.deactivate();
        }

        // check state, activate if needed
        if self.plugin.status() == PluginStatus::Deactivated {
            self.plugin.shared().requested_restart.store(false);

            let min_buffer_size = self.min_buffer_size;
            let sample_rate = self.sample_rate;
            let buffer_size = self.buffer.samples();

            self.plugin
                .on_main_thread(move |plugin| plugin.activate(sample_rate, min_buffer_size, buffer_size))?;
        }

        // start processing if needed
        if self.plugin.status() == PluginStatus::Activated {
            self.plugin.start_processing()?;
        }

        Ok(())
    }

    /// Deactivate/stop processing if needed.
    ///
    /// The state will be [`PluginStatus::Deactivated`] if successful.
    pub fn deactivate(&mut self) {
        self.plugin.shared().requested_restart.store(false);

        if self.plugin.status() == PluginStatus::Processing {
            self.plugin.stop_processing();
        }

        if self.plugin.status() == PluginStatus::Activated {
            self.plugin.on_main_thread(|plugin| plugin.deactivate());
        }
    }
}

impl Drop for ProcessScope<'_> {
    fn drop(&mut self) {
        self.deactivate();
    }
}

/// NaN values used for checking if output buffers have been written to.
/// These are quiet NaNs with a specific payload to avoid accidental matches with other NaN values.
/// The payload is chosen to be unlikely to appear in normal processing.
const CHECK_NAN_F32: f32 = f32::from_bits(0x7FC0_1234);
/// See [`CHECK_NAN_F32`].
const CHECK_NAN_F64: f64 = f64::from_bits(0x7FF8_1234_5678_1234);

/// The process for consistency. This verifies that the output buffer has been written to, doesn't contain any NaN,
/// infinite, or denormal values, that the input buffers have not been modified by the plugin, and
/// that the output event queue is monotonically ordered.
fn check_process_call_consistency(
    resulting_buffers: &[AudioBuffer],
    original_buffers: &[AudioBuffer],
    output_events: &[Event],
    block_size: u32,
    check_denormals: bool,
    check_outputs: &[bool],
) -> Result<()> {
    for (buffer, before) in resulting_buffers.iter().zip(original_buffers.iter()) {
        // Input-only buffers must not be overwritten during out of place processing
        match buffer.port() {
            AudioBufferPort::Input(index) => {
                // find a mismatching sample
                for channel in 0..buffer.channels() {
                    for sample in 0..buffer.samples() {
                        let x = buffer.get(channel, sample);
                        let y = before.get(channel, sample);

                        anyhow::ensure!(
                            x == y,
                            "The plugin has overwritten an input buffer (index {index}) during out-of-place \
                             processing, at channel {channel} and sample index {sample}."
                        );
                    }
                }
            }

            // Output buffers must not contain any non-finite or denormal values
            AudioBufferPort::Output(port_idx) | AudioBufferPort::Inplace(_, port_idx) => {
                if !check_outputs.get(port_idx).copied().unwrap_or(false) {
                    continue;
                }

                // check output constant masks
                for channel in 0..buffer.channels() {
                    if buffer.get_output_constant_mask().is_channel_constant(channel)
                        && let Err(e) = check_channel_quiet(buffer.channel(channel), true)
                    {
                        anyhow::bail!(
                            "The output channel {channel} of port {port_idx} is not constant despite the constant \
                             flag being set ({e:.2} dBFS)."
                        );
                    }
                }

                // check for invalid samples (unwritten, NaN, infinite, or denormal)
                let invalid_sample = (0..buffer.channels())
                    .flat_map(|channel| (0..block_size).map(move |sample| (channel, sample)))
                    .find_map(|(channel, sample)| {
                        let x = buffer.get(channel, sample);
                        if x.either(
                            |x| !x.is_finite() || (x.is_subnormal() && check_denormals),
                            |x| !x.is_finite() || (x.is_subnormal() && check_denormals),
                        ) {
                            Some((x, channel, sample))
                        } else {
                            None
                        }
                    });

                if let Some((sample, channel_idx, sample_idx)) = invalid_sample {
                    let is_subnormal = sample.either(|x| x.is_subnormal(), |x| x.is_subnormal());
                    let is_unwritten = sample.either(
                        |x| x.to_bits() == CHECK_NAN_F32.to_bits(),
                        |x| x.to_bits() == CHECK_NAN_F64.to_bits(),
                    );

                    if is_subnormal {
                        anyhow::bail!(
                            "The sample written to output port {port_idx}, channel {channel_idx}, and sample index \
                             {sample_idx} is subnormal ({sample})."
                        );
                    } else if is_unwritten {
                        anyhow::bail!(
                            "The sample at output port {port_idx}, channel {channel_idx}, and sample index \
                             {sample_idx} was left unwritten."
                        );
                    } else {
                        anyhow::bail!(
                            "The sample written to output port {port_idx}, channel {channel_idx}, and sample index \
                             {sample_idx} is {sample}."
                        );
                    }
                }

                // check for out-of-bounds overwritten samples
                let overwritten_sample = (0..buffer.channels())
                    .flat_map(|channel| (block_size..buffer.samples()).map(move |sample| (channel, sample)))
                    .find_map(|(channel, sample)| {
                        let bitwise_match = match (buffer.get(channel, sample), before.get(channel, sample)) {
                            (Either::Left(x), Either::Left(y)) => x.to_bits() == y.to_bits(),
                            (Either::Right(x), Either::Right(y)) => x.to_bits() == y.to_bits(),
                            _ => false,
                        };

                        if !bitwise_match { Some((channel, sample)) } else { None }
                    });

                if let Some((channel_idx, sample_idx)) = overwritten_sample {
                    anyhow::bail!(
                        "The plugin has overwritten a sample beyond the current processing block size at channel \
                         {channel_idx} and sample index {sample_idx}. The block size is {block_size}."
                    );
                }
            }
        }
    }

    // If the plugin output any events, then they should be in a monotonically increasing order
    let mut last_event_time = 0;
    for event in output_events {
        let event_time = event.header().time;
        if event_time < last_event_time {
            anyhow::bail!(
                "The plugin output an event for sample {event_time} after it had previously output an event for \
                 sample {last_event_time}."
            )
        }

        if event_time >= block_size {
            anyhow::bail!(
                "The plugin output an event for sample {} but the audio buffer only contains {} samples.",
                event_time,
                block_size
            )
        }

        if matches!(event, Event::Transport(_)) {
            anyhow::bail!("The plugin emitted a transport event during processing, which is not allowed.");
        }

        last_event_time = event_time;
    }

    Ok(())
}

/// A channel is considered quiet if the signal is below -60 dbfs, ignoring DC.
///
/// This function is designed to be very lenient in what it considers "quiet", to avoid false positives.
/// Returns `Ok(())` if the channel is quiet, or `Err(max_amplitude_in_db)` if not.
pub fn check_channel_quiet(channel: Either<&[f32], &[f64]>, ignore_dc: bool) -> Result<(), f64> {
    /// -60 dbfs
    const QUIET_THRESHOLD: f64 = 0.001;

    let (min, max) = match channel {
        Either::Right(x) => x.iter().fold((f64::MAX, f64::MIN), |(min, max), &sample| {
            (min.min(sample.abs()), max.max(sample.abs()))
        }),
        Either::Left(x) => {
            let (min, max) = x.iter().fold((f32::MAX, f32::MIN), |(min, max), &sample| {
                (min.min(sample.abs()), max.max(sample.abs()))
            });

            (min as f64, max as f64)
        }
    };

    let range = if ignore_dc { (max - min) * 0.5 } else { max.max(-min) };

    if range < QUIET_THRESHOLD {
        Ok(())
    } else {
        Err(20.0 * range.log10())
    }
}
