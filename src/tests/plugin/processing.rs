//! Contains most of the boilerplate around testing audio processing.

use anyhow::{Context, Result};

use crate::plugin::audio_thread::process::{
    AudioBuffers, OutOfPlaceAudioBuffers, ProcessConfig, ProcessData,
};
use crate::plugin::instance::Plugin;

/// A helper to handle the boilerplate that comes with testing a plugin's audio processing behavior.
pub struct ProcessingTest<'a> {
    plugin: &'a Plugin<'a>,
    audio_buffers: AudioBuffers<'a>,
}

impl<'a> ProcessingTest<'a> {
    /// Construct a new processing test using out-of-place processing. This allocates the CLAP audio
    /// buffer structs needed for the test. Returns an error if the the inner vectors don't all have
    /// the same length.
    pub fn new_out_of_place(
        plugin: &'a Plugin<'a>,
        input_buffers: &'a mut [Vec<Vec<f32>>],
        output_buffers: &'a mut [Vec<Vec<f32>>],
    ) -> Result<Self> {
        Ok(Self {
            plugin,
            audio_buffers: AudioBuffers::OutOfPlace(OutOfPlaceAudioBuffers::new(
                input_buffers,
                output_buffers,
            )?),
        })
    }

    /// Run the standard audio processing test for a still **deactivated** plugin. This calls the
    /// process function `num_iters` times, and checks the output for consistency each time.
    ///
    /// The `Preprocess` closure is called before each processing cycle to allow the process data to be
    /// modified for the next process cycle.
    pub fn run<Preprocess>(
        &'a mut self,
        num_iters: usize,
        process_config: ProcessConfig,
        mut preprocess: Preprocess,
    ) -> Result<()>
    where
        Preprocess: FnMut(&mut ProcessData) -> Result<()> + Send,
    {
        let buffer_size = self.audio_buffers.len();
        let mut process_data = ProcessData::new(&mut self.audio_buffers, process_config);

        self.plugin
            .activate(process_config.sample_rate, 1, buffer_size)?;

        self.plugin.on_audio_thread(|plugin| -> Result<()> {
            plugin.start_processing()?;

            // This test can be repeated a couple of times
            // NOTE: We intentionally do not disable denormals here
            for iteration in 0..num_iters {
                preprocess(&mut process_data)?;

                // We'll check that the plugin hasn't modified the input buffers after the
                // test
                let original_input_buffers = process_data.buffers.inputs_ref().to_owned();

                plugin
                    .process(&mut process_data)
                    .context("Error during audio processing")?;

                // When we add in-place processing this will need some slightly different checks
                match process_data.buffers {
                    AudioBuffers::OutOfPlace(_) => check_out_of_place_output_consistency(
                        &process_data,
                        &original_input_buffers,
                    ),
                }
                .with_context(|| {
                    format!(
                        "Failed during processing cycle {} out of {}",
                        iteration + 1,
                        num_iters
                    )
                })?;

                process_data.clear_events();
                process_data.advance_transport(buffer_size as u32);
            }

            plugin.stop_processing()
        })?;

        self.plugin.deactivate()
    }
}

/// The process for consistency. This verifies that the output buffer doesn't contain any NaN,
/// infinite, or denormal values, that the input buffers have not been modified by the plugin, and
/// that the output event queue is monotonically ordered.
fn check_out_of_place_output_consistency(
    process_data: &ProcessData,
    original_input_buffers: &[Vec<Vec<f32>>],
) -> Result<()> {
    // The input buffer must not be overwritten during out of place processing, and the outputs
    // should not contain any non-finite or denormal values
    let input_buffers = process_data.buffers.inputs_ref();
    let output_buffers = process_data.buffers.outputs_ref();
    if input_buffers != original_input_buffers {
        anyhow::bail!(
            "The plugin has overwritten the input buffers during out-of-place processing"
        );
    }
    for (port_idx, channel_slices) in output_buffers.iter().enumerate() {
        for (channel_idx, channel_slice) in channel_slices.iter().enumerate() {
            for (sample_idx, sample) in channel_slice.iter().enumerate() {
                if !sample.is_finite() {
                    anyhow::bail!("The sample written to output port {port_idx}, channel {channel_idx}, and sample index {sample_idx} is {sample:?}");
                } else if sample.is_subnormal() {
                    anyhow::bail!("The sample written to output port {port_idx}, channel {channel_idx}, and sample index {sample_idx} is subnormal ({sample:?})");
                }
            }
        }
    }

    // If the plugin output any events, then they should be in a monotonically increasing order
    let mut last_event_time = 0;
    #[allow(clippy::significant_drop_in_scrutinee)]
    for event in process_data.output_events.events.lock().unwrap().iter() {
        let event_time = event.header().time;
        if event_time < last_event_time {
            anyhow::bail!("The plugin output an event for sample {event_time} after it had previously output an event for sample {last_event_time}")
        }

        last_event_time = event_time;
    }

    Ok(())
}
