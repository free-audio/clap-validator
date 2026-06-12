use crate::plugin::ext::audio_ports::{AudioPort, AudioPortConfig};
use crate::plugin::process::ConstantMask;
use anyhow::Result;
use clap_sys::audio_buffer::*;
use either::Either;
use rand::{Rng, RngExt};
use std::collections::HashMap;
use std::fmt::Debug;
use std::mem::zeroed;
use std::ops::{Deref, DerefMut};
use std::ptr::null_mut;

/// Audio buffers for audio processing. These contain both input and output buffers, that can be either in-place
/// or out-of-place, single or double precision.
#[derive(Clone)]
pub struct AudioBuffers {
    /// These are all indexed by `[port_idx][channel_idx][sample_idx]`. The inputs also need to be
    /// mutable because reborrwing them from here is the only way to modify them without
    /// reinitializing the pointers.
    buffers: Box<[AudioBuffer]>,

    /// The CLAP audio buffer representations for inputs
    clap_inputs: Box<[clap_audio_buffer]>,
    /// The CLAP audio buffer representations for outputs
    clap_outputs: Box<[clap_audio_buffer]>,

    ptrs_inputs: Box<[Box<[*mut ()]>]>,
    ptrs_outputs: Box<[Box<[*mut ()]>]>,

    /// The number of samples for this buffer. This is consistent across all inner vectors.
    samples: u32,
}

#[derive(Debug, Clone)]
pub struct AudioBuffer {
    port: AudioBufferPort,

    input_constant_mask: ConstantMask,
    output_constant_mask: ConstantMask,

    input_latency: u32,
    output_latency: u32,

    #[allow(clippy::type_complexity)]
    data: Either<Box<[Box<[f32]>]>, Box<[Box<[f64]>]>>,
    samples: u32,
}

/// A port to which an audio buffer belongs.
#[derive(Clone, Copy, Debug)]
pub enum AudioBufferPort {
    Input(usize),
    Output(usize),
    Inplace(usize, usize),
}

impl AudioBuffers {
    /// Construct the audio buffers from the given buffer configurations. The number of samples must
    /// be greater than zero and all channel vectors must have the same length.
    pub fn new(buffers: Vec<AudioBuffer>, samples: u32) -> Self {
        let mut clap_inputs: Vec<clap_audio_buffer> = vec![];
        let mut clap_outputs: Vec<clap_audio_buffer> = vec![];
        let mut ptrs_inputs: Vec<Box<[*mut ()]>> = vec![];
        let mut ptrs_outputs: Vec<Box<[*mut ()]>> = vec![];

        for buffer in buffers.iter() {
            assert!(
                buffer.samples() == samples,
                "All audio buffers must have the same number of samples."
            );

            if let Some(input) = buffer.port().input() {
                if clap_inputs.len() <= input {
                    clap_inputs.resize(input + 1, unsafe { zeroed() });
                    ptrs_inputs.resize(input + 1, Box::new([]));
                }

                ptrs_inputs[input] = vec![null_mut(); buffer.channels() as usize].into_boxed_slice();
            }

            if let Some(output) = buffer.port().output() {
                if clap_outputs.len() <= output {
                    clap_outputs.resize(output + 1, unsafe { zeroed() });
                    ptrs_outputs.resize(output + 1, Box::new([]));
                }

                ptrs_outputs[output] = vec![null_mut(); buffer.channels() as usize].into_boxed_slice();
            }
        }

        Self {
            clap_inputs: clap_inputs.into_boxed_slice(),
            clap_outputs: clap_outputs.into_boxed_slice(),
            ptrs_inputs: ptrs_inputs.into_boxed_slice(),
            ptrs_outputs: ptrs_outputs.into_boxed_slice(),
            buffers: buffers.into_boxed_slice(),
            samples,
        }
    }

    pub fn new_out_of_place_f32(config: &AudioPortConfig, samples: u32) -> Self {
        Self::new(
            (0..config.inputs.len())
                .map(AudioBufferPort::Input)
                .chain((0..config.outputs.len()).map(AudioBufferPort::Output))
                .map(|port| port.create_buffer(config, samples, false))
                .collect(),
            samples,
        )
    }

    pub fn new_out_of_place_f64(config: &AudioPortConfig, samples: u32) -> Self {
        Self::new(
            (0..config.inputs.len())
                .map(AudioBufferPort::Input)
                .chain((0..config.outputs.len()).map(AudioBufferPort::Output))
                .map(|port| port.create_buffer(config, samples, true))
                .collect(),
            samples,
        )
    }

    pub fn new_in_place_f32(config: &AudioPortConfig, samples: u32) -> Result<Self> {
        Ok(Self::new(
            resolve_in_place_pairs(config)?
                .iter()
                .map(|port| port.create_buffer(config, samples, false))
                .collect(),
            samples,
        ))
    }

    pub fn new_in_place_f64(config: &AudioPortConfig, samples: u32) -> Result<Self> {
        Ok(Self::new(
            resolve_in_place_pairs(config)?
                .iter()
                .map(|port| port.create_buffer(config, samples, true))
                .collect(),
            samples,
        ))
    }

    #[allow(clippy::obfuscated_if_else)]
    pub fn process<R>(
        &mut self,
        f: impl FnOnce(&[clap_audio_buffer], &mut [clap_audio_buffer]) -> Result<R>,
    ) -> Result<R> {
        for buffer in self.buffers.iter() {
            if let Some(input) = buffer.port().input() {
                let clap = &mut self.clap_inputs[input];
                let ptrs = &mut self.ptrs_inputs[input];

                clap.data32 = buffer.is_32bit().then_some(ptrs.as_mut_ptr()).unwrap_or_default() as *mut _;
                clap.data64 = buffer.is_64bit().then_some(ptrs.as_mut_ptr()).unwrap_or_default() as *mut _;
                clap.channel_count = buffer.channels();
                clap.constant_mask = buffer.input_constant_mask.0;
                clap.latency = buffer.input_latency;

                for i in 0..buffer.channels() as usize {
                    ptrs[i] = buffer.channel_ptr(i as u32);
                }
            }

            if let Some(output) = buffer.port().output() {
                let clap = &mut self.clap_outputs[output];
                let ptrs = &mut self.ptrs_outputs[output];

                clap.data32 = buffer.is_32bit().then_some(ptrs.as_mut_ptr()).unwrap_or_default() as *mut _;
                clap.data64 = buffer.is_64bit().then_some(ptrs.as_mut_ptr()).unwrap_or_default() as *mut _;
                clap.channel_count = buffer.channels();
                clap.constant_mask = 0;
                clap.latency = 0;

                for i in 0..buffer.channels() as usize {
                    ptrs[i] = buffer.channel_ptr(i as u32);
                }
            }
        }

        let result = f(&self.clap_inputs, &mut self.clap_outputs)?;

        for buffer in self.buffers.iter_mut() {
            if let Some(input) = buffer.port().input() {
                let clap = &self.clap_inputs[input];
                let ptrs = &self.ptrs_inputs[input];

                let ptr32 = buffer.is_32bit().then_some(ptrs.as_ptr()).unwrap_or_default() as *mut _;
                let ptr64 = buffer.is_64bit().then_some(ptrs.as_ptr()).unwrap_or_default() as *mut _;

                if clap.data32 != ptr32
                    || clap.data64 != ptr64
                    || clap.channel_count != buffer.channels()
                    || clap.constant_mask != buffer.input_constant_mask.0
                    || clap.latency != buffer.input_latency
                {
                    anyhow::bail!(
                        "The plugin modified the input buffer (index {input}) data while processing, which is not \
                         allowed."
                    );
                }

                for i in 0..buffer.channels() as usize {
                    if ptrs[i] != buffer.channel_ptr(i as u32) {
                        anyhow::bail!(
                            "The plugin modified the input buffer (index {input}) channel pointers while processing, \
                             which is not allowed."
                        );
                    }
                }
            }

            if let Some(output) = buffer.port().output() {
                let clap = &self.clap_outputs[output];
                let ptrs = &self.ptrs_outputs[output];

                let ptr32 = buffer.is_32bit().then_some(ptrs.as_ptr()).unwrap_or_default() as *mut _;
                let ptr64 = buffer.is_64bit().then_some(ptrs.as_ptr()).unwrap_or_default() as *mut _;

                if clap.data32 != ptr32 || clap.data64 != ptr64 || clap.channel_count != buffer.channels() {
                    anyhow::bail!(
                        "The plugin modified the output buffer (index {output}) data while processing, which is not \
                         allowed."
                    );
                }

                for i in 0..buffer.channels() as usize {
                    if ptrs[i] != buffer.channel_ptr(i as u32) {
                        anyhow::bail!(
                            "The plugin modified the output buffer (index {output}) channel pointers while \
                             processing, which is not allowed."
                        );
                    }
                }

                buffer.output_constant_mask = ConstantMask(clap.constant_mask);
                buffer.output_latency = clap.latency;
            }
        }

        Ok(result)
    }

    pub fn samples(&self) -> u32 {
        self.samples
    }

    pub fn fill_white_noise(&mut self, prng: &mut impl Rng) {
        for buffer in self.buffers.iter_mut() {
            if buffer.port().input().is_some() {
                buffer.fill_white_noise(prng);
            }
        }
    }

    pub fn fill_silence(&mut self) {
        for buffer in self.buffers.iter_mut() {
            if buffer.port().input().is_some() {
                buffer.fill_silence();
            }
        }
    }

    pub fn num_outputs(&self) -> usize {
        self.clap_outputs.len()
    }
}

impl AudioBuffer {
    pub fn new(port: AudioBufferPort, channels: u32, samples: u32, is_double: bool) -> Self {
        let data = if is_double {
            Either::Right(vec![vec![0.0f64; samples as usize].into_boxed_slice(); channels as usize].into_boxed_slice())
        } else {
            Either::Left(vec![vec![0.0f32; samples as usize].into_boxed_slice(); channels as usize].into_boxed_slice())
        };

        Self {
            port,
            data,
            samples,
            input_constant_mask: ConstantMask::DYNAMIC,
            output_constant_mask: ConstantMask::DYNAMIC,
            input_latency: 0,
            output_latency: 0,
        }
    }

    pub fn port(&self) -> AudioBufferPort {
        self.port
    }

    pub fn set_input_constant_mask(&mut self, mask: ConstantMask) {
        self.input_constant_mask = mask;
    }

    pub fn get_output_constant_mask(&self) -> ConstantMask {
        self.output_constant_mask
    }

    #[allow(unused)]
    pub fn set_input_latency(&mut self, latency: u32) {
        self.input_latency = latency;
    }

    #[allow(unused)]
    pub fn get_output_latency(&self) -> u32 {
        self.output_latency
    }

    pub fn fill_white_noise(&mut self, prng: &mut impl Rng) {
        for channel in 0..self.channels() {
            match self.channel_mut(channel) {
                Either::Left(data) => data.fill_with(|| prng.random_range(-1.0..1.0)),
                Either::Right(data) => data.fill_with(|| prng.random_range(-1.0..1.0)),
            }
        }

        self.set_input_constant_mask(ConstantMask::DYNAMIC);
    }

    pub fn fill_silence(&mut self) {
        self.fill(0.0, 0.0);
        self.set_input_constant_mask(ConstantMask::CONSTANT);
    }

    pub fn fill(&mut self, value_f32: f32, value_f64: f64) {
        for channel in 0..self.channels() {
            match self.channel_mut(channel) {
                Either::Left(data) => data.fill(value_f32),
                Either::Right(data) => data.fill(value_f64),
            }
        }
    }

    pub fn is_64bit(&self) -> bool {
        self.data.is_right()
    }

    pub fn is_32bit(&self) -> bool {
        self.data.is_left()
    }

    pub fn samples(&self) -> u32 {
        self.samples
    }

    pub fn channels(&self) -> u32 {
        match &self.data {
            Either::Left(data) => data.len() as u32,
            Either::Right(data) => data.len() as u32,
        }
    }

    pub fn channel(&self, channel: u32) -> Either<&[f32], &[f64]> {
        match &self.data {
            Either::Left(data) => Either::Left(&data[channel as usize]),
            Either::Right(data) => Either::Right(&data[channel as usize]),
        }
    }

    pub fn channel_mut(&mut self, channel: u32) -> Either<&mut [f32], &mut [f64]> {
        match &mut self.data {
            Either::Left(data) => Either::Left(&mut data[channel as usize]),
            Either::Right(data) => Either::Right(&mut data[channel as usize]),
        }
    }

    pub fn channel_ptr(&self, channel: u32) -> *mut () {
        match &self.data {
            Either::Left(data) => data[channel as usize].as_ptr() as *mut (),
            Either::Right(data) => data[channel as usize].as_ptr() as *mut (),
        }
    }

    pub fn get(&self, channel: u32, sample: u32) -> Either<f32, f64> {
        match &self.data {
            Either::Left(data) => Either::Left(data[channel as usize][sample as usize]),
            Either::Right(data) => Either::Right(data[channel as usize][sample as usize]),
        }
    }
}

impl AudioBufferPort {
    pub fn input(&self) -> Option<usize> {
        match self {
            AudioBufferPort::Input(index) => Some(*index),
            AudioBufferPort::Inplace(index, _) => Some(*index),
            AudioBufferPort::Output(_) => None,
        }
    }

    pub fn output(&self) -> Option<usize> {
        match self {
            AudioBufferPort::Output(index) => Some(*index),
            AudioBufferPort::Inplace(_, index) => Some(*index),
            AudioBufferPort::Input(_) => None,
        }
    }

    pub fn create_buffer(self, config: &AudioPortConfig, samples: u32, is_double: bool) -> AudioBuffer {
        match self {
            AudioBufferPort::Input(index) => AudioBuffer::new(
                self,
                config.inputs[index].channel_count,
                samples,
                is_double && config.inputs[index].supports_double_sample_size,
            ),
            AudioBufferPort::Output(index) => AudioBuffer::new(
                self,
                config.outputs[index].channel_count,
                samples,
                is_double && config.outputs[index].supports_double_sample_size,
            ),
            AudioBufferPort::Inplace(input_index, output_index) => AudioBuffer::new(
                self,
                config.inputs[input_index].channel_count,
                samples,
                is_double
                    && config.inputs[input_index].supports_double_sample_size
                    && config.outputs[output_index].supports_double_sample_size,
            ),
        }
    }
}

unsafe impl Send for AudioBuffers {}
unsafe impl Sync for AudioBuffers {}

impl Deref for AudioBuffers {
    type Target = [AudioBuffer];
    fn deref(&self) -> &Self::Target {
        &self.buffers
    }
}

impl DerefMut for AudioBuffers {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.buffers
    }
}

/// Resolve the in-place pairs from the given audio port configuration.
///
/// Returns an error if there are any inconsistencies, such as an input or output port
/// referencing a non-existent in-place pair.
fn resolve_in_place_pairs(config: &AudioPortConfig) -> Result<Vec<AudioBufferPort>> {
    fn is_same_layout(port: &AudioPort, other: &AudioPort) -> bool {
        port.channel_count == other.channel_count
            && port.port_type == other.port_type
            && port.supports_double_sample_size == other.supports_double_sample_size
            && port.requires_common_sample_size == other.requires_common_sample_size
            && port.prefers_double_sample_size == other.prefers_double_sample_size
    }

    let mut ports = vec![];
    let mut in_place: HashMap<(u32, u32), (Option<usize>, Option<usize>)> = HashMap::new();

    for (index, port) in config.inputs.iter().enumerate() {
        if let Some(inplace_id) = port.in_place_pair {
            in_place.entry((port.id, inplace_id)).or_default().0 = Some(index);
        } else {
            ports.push(AudioBufferPort::Input(index));
        }
    }

    for (index, port) in config.outputs.iter().enumerate() {
        if let Some(inplace_id) = port.in_place_pair {
            in_place.entry((inplace_id, port.id)).or_default().1 = Some(index);
        } else {
            ports.push(AudioBufferPort::Output(index));
        }
    }

    for ((input_id, output_id), (input, output)) in in_place {
        match (input, output) {
            (None, Some(output)) => anyhow::bail!(
                "Output port {output} has an in-place pair ({input_id}), but the corresponding input port does not \
                 exist."
            ),
            (Some(input), None) => anyhow::bail!(
                "Input port {input} has an in-place pair ({output_id}), but the corresponding output port does not \
                 exist."
            ),
            (Some(input), Some(output)) => {
                if !is_same_layout(&config.inputs[input], &config.outputs[output]) {
                    anyhow::bail!(
                        "Input port {input} and output port {output} are configured as an in-place pair, but they \
                         have different flags/layouts.",
                    );
                }

                ports.push(AudioBufferPort::Inplace(input, output));
            }
            _ => {}
        }
    }

    Ok(ports)
}
