use crate::plugin::process::{AudioBuffers, ConstantMask};
use either::Either;
use rand::seq::IndexedRandom;
use rand::{Rng, RngExt, SeedableRng};
use std::f64::consts::TAU;

/// Creates a new PRNG that is seeded with the current time.
///
/// Used for generating the seeds for child PRNGs
pub fn new_orchestrator_prng() -> rand::rngs::Xoshiro128PlusPlus {
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();

    rand::rngs::Xoshiro128PlusPlus::from_seed(time.to_le_bytes())
}

pub fn random_sample_rate(rng: &mut impl Rng) -> f64 {
    const PRESET: &[f64] = &[8000.0, 11025.0, 22050.0, 44100.0, 48000.0, 96000.0, 192000.0, 384000.0];

    if rng.random_bool(0.1) {
        rng.random_range(1000.0..200000.0)
    } else {
        *PRESET.choose(rng).unwrap()
    }
}

pub fn random_buffer_size_range(rng: &mut impl Rng) -> (u32, u32) {
    let max_buffer_size = {
        const PRESET: &[u32] = &[64, 128, 256, 512, 1024, 2048, 4096, 16384];

        if rng.random_bool(0.1) {
            rng.random_range(1..10000)
        } else {
            *PRESET.choose(rng).unwrap()
        }
    };

    let min_buffer_size = if rng.random_bool(0.25) {
        max_buffer_size
    } else if rng.random_bool(0.25) {
        1
    } else {
        rng.random_range(1..=max_buffer_size)
    };

    (min_buffer_size, max_buffer_size)
}

pub struct AudioFuzzer {
    generators: Vec<Vec<AudioSignal>>,
    change_probability: f64,
}

impl AudioFuzzer {
    pub fn new() -> Self {
        Self {
            generators: vec![],
            change_probability: 0.1,
        }
    }

    pub fn fill(&mut self, rng: &mut impl Rng, sample_rate: f64, buffers: &mut AudioBuffers) {
        for buffer in buffers.iter_mut() {
            let Some(input) = buffer.port().input() else { continue };

            if self.generators.len() <= input {
                self.generators.resize_with(input + 1, Vec::new);
            }

            let signals = &mut self.generators[input];
            if signals.len() < buffer.channels() as usize {
                signals.resize_with(buffer.channels() as usize, || AudioSignal::rng(rng, sample_rate));
            }

            let mut constant_mask = ConstantMask::DYNAMIC;
            for (channel, generator) in signals.iter_mut().enumerate() {
                if rng.random_bool(self.change_probability) {
                    *generator = AudioSignal::rng(rng, sample_rate);
                }

                generator.fill(rng, sample_rate, buffer.channel_mut(channel as u32));

                if generator.is_constant() {
                    constant_mask = constant_mask.with_channel_constant(channel as u32);
                }
            }

            buffer.set_input_constant_mask(constant_mask);
        }
    }
}

pub enum AudioSignal {
    Sine {
        phase: f64,
        freq: f64,
        freq_ramp: f64,
        gain: f64,
        gain_ramp: f64,
    },

    Noise {
        gain: f64,
        gain_ramp: f64,
    },

    Constant {
        value: f64,
    },

    Denormal,
}

impl AudioSignal {
    pub fn rng(rng: &mut impl Rng, sample_rate: f64) -> Self {
        match rng.random_range(0..7) {
            // sine at nyquist
            0 => Self::Sine {
                phase: 0.5,
                freq: sample_rate / 2.0,
                freq_ramp: 0.0,
                gain: 0.0,
                gain_ramp: rng.random_range(-10.0..10.0),
            },

            // sine at near dc
            1 => Self::Sine {
                phase: 0.5,
                freq: 1.0,
                freq_ramp: 0.0,
                gain: 0.0,
                gain_ramp: rng.random_range(-10.0..10.0),
            },

            // random sine sweep
            2 => Self::Sine {
                phase: rng.random_range(0.0..1.0),
                freq: rng.random_range(20.0..20000.0),
                freq_ramp: rng.random_range(-1000.0..1000.0),
                gain: rng.random_range(-80.0..20.0),
                gain_ramp: rng.random_range(-10.0..10.0),
            },

            // random noise ramp
            3 => Self::Noise {
                gain: rng.random_range(-80.0..20.0),
                gain_ramp: rng.random_range(-10.0..10.0),
            },

            // constant signal between -1 and 1
            4 => Self::Constant {
                value: rng.random_range(-1.0..1.0),
            },

            // constant signal (silent)
            5 => Self::Constant { value: 0.0 },

            _ => Self::Denormal,
        }
    }

    pub fn fill(&mut self, rng: &mut impl Rng, sample_rate: f64, buffer: Either<&mut [f32], &mut [f64]>) {
        fn db_to_gain(db: f64) -> f64 {
            const MAX_GAIN_DB: f64 = 40.0;
            10f64.powf(db.min(MAX_GAIN_DB) / 20.0)
        }

        let sample_rate_inv = 1.0 / sample_rate;

        match self {
            AudioSignal::Sine {
                phase,
                freq,
                freq_ramp,
                gain,
                gain_ramp,
            } => match buffer {
                Either::Left(buf) => {
                    for sample in buf {
                        *freq += *freq_ramp * sample_rate_inv;
                        *gain += *gain_ramp * sample_rate_inv;
                        *phase = (*phase + *freq * sample_rate_inv).rem_euclid(1.0);
                        *sample = (*phase * TAU).sin() as f32 * db_to_gain(*gain) as f32;
                    }
                }

                Either::Right(buf) => {
                    for sample in buf {
                        *freq += *freq_ramp * sample_rate_inv;
                        *gain += *gain_ramp * sample_rate_inv;
                        *phase = (*phase + *freq * sample_rate_inv).rem_euclid(1.0);
                        *sample = (*phase * TAU).sin() * db_to_gain(*gain);
                    }
                }
            },

            AudioSignal::Noise { gain, gain_ramp } => match buffer {
                Either::Left(buf) => {
                    for sample in buf {
                        *gain += *gain_ramp * sample_rate_inv;
                        *sample = rng.random_range(-1.0..1.0) * db_to_gain(*gain) as f32;
                    }
                }

                Either::Right(buf) => {
                    for sample in buf {
                        *gain += *gain_ramp * sample_rate_inv;
                        *sample = rng.random_range(-1.0..1.0) * db_to_gain(*gain);
                    }
                }
            },

            AudioSignal::Constant { value } => match buffer {
                Either::Left(buf) => {
                    for sample in buf {
                        *sample = *value as f32;
                    }
                }

                Either::Right(buf) => {
                    for sample in buf {
                        *sample = *value;
                    }
                }
            },

            AudioSignal::Denormal => match buffer {
                Either::Left(buf) => {
                    for sample in buf {
                        *sample = rng.random_range(-f32::MIN_POSITIVE..f32::MIN_POSITIVE);
                    }
                }

                Either::Right(buf) => {
                    for sample in buf {
                        *sample = rng.random_range(-f64::MIN_POSITIVE..f64::MIN_POSITIVE);
                    }
                }
            },
        }
    }

    pub fn is_constant(&self) -> bool {
        matches!(self, Self::Constant { .. })
    }
}
