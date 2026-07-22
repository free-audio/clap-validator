//! Contains most of the boilerplate around testing audio processing.

use crate::cli::tracing::{Span, record};
use crate::plugin::ext::audio_ports::{AudioPortConfig, AudioPorts};
use crate::plugin::ext::note_ports::{NotePortConfig, NotePorts};
use crate::plugin::ext::tail::Tail;
use crate::plugin::ext::voice_info::VoiceInfo;
use crate::plugin::instance::{CallbackEvent, ProcessStatus};
use crate::plugin::library::PluginLibrary;
use crate::plugin::process::{AudioBuffers, ConstantMask, ProcessScope, check_channel_quiet};
use crate::tests::TestStatus;
use crate::tests::rng::{NoteGenerator, new_prng};
use anyhow::{Context, Result};
use clap_sys::ext::voice_info::CLAP_VOICE_INFO_SUPPORTS_OVERLAPPING_NOTES;
use either::Either;
use rand::RngExt;
use std::time::Instant;

const BUFFER_SIZE: u32 = 512;

/// The test for `PluginTestCase::ProcessAudioOutOfPlaceBasic` and `PluginTestCase::ProcessAudioInPlaceBasic`.
pub fn test_process_audio_basic(library: &PluginLibrary, plugin_id: &str, in_place: bool) -> Result<TestStatus> {
    let mut prng = new_prng();

    let plugin = library
        .create_plugin(plugin_id)
        .context("Could not create the plugin instance")?;
    plugin.init().context("Error during initialization")?;

    let audio_ports_config = match plugin.get_extension::<AudioPorts>() {
        Some(audio_ports) => audio_ports
            .config()
            .context("Error while querying 'audio-ports' IO configuration")?,
        None => {
            return Ok(TestStatus::Skipped {
                details: Some(String::from(
                    "The plugin does not implement the 'audio-ports' extension.",
                )),
            });
        }
    };

    let mut audio_buffers = if in_place {
        AudioBuffers::new_in_place_f32(&audio_ports_config, BUFFER_SIZE)?
    } else {
        AudioBuffers::new_out_of_place_f32(&audio_ports_config, BUFFER_SIZE)
    };

    plugin.on_audio_thread(|plugin| -> Result<()> {
        let mut process = ProcessScope::new(&plugin, &mut audio_buffers)?;

        for _ in 0..5 {
            plugin.poll_callback();
            process.audio_buffers().fill_white_noise(&mut prng);
            process.run()?;
        }

        Ok(())
    })?;

    plugin.poll_callback(|_| Ok(()))?;

    Ok(TestStatus::Success { details: None })
}

// The test for `PluginTestCase::ProcessAudioOutOfPlaceDouble`.
pub fn test_process_audio_double(library: &PluginLibrary, plugin_id: &str, in_place: bool) -> Result<TestStatus> {
    let mut prng = new_prng();

    let plugin = library
        .create_plugin(plugin_id)
        .context("Could not create the plugin instance")?;
    plugin.init().context("Error during initialization")?;

    let audio_ports_config = match plugin.get_extension::<AudioPorts>() {
        Some(audio_ports) => audio_ports
            .config()
            .context("Error while querying 'audio-ports' IO configuration")?,
        None => {
            return Ok(TestStatus::Skipped {
                details: Some(String::from(
                    "The plugin does not implement the 'audio-ports' extension.",
                )),
            });
        }
    };

    let note_ports_config = plugin
        .get_extension::<NotePorts>()
        .map(|x| x.config())
        .transpose()
        .context("Error while querying 'note-ports' IO configuration")?
        .unwrap_or_default();

    plugin.poll_callback(|_| Ok(()))?;

    let has_double_support = audio_ports_config
        .inputs
        .iter()
        .chain(audio_ports_config.outputs.iter())
        .any(|port| port.supports_double_sample_size);

    if !has_double_support {
        return Ok(TestStatus::Skipped {
            details: Some(String::from("The plugin does not support 64-bit floating point audio.")),
        });
    }

    let mut note_rng = NoteGenerator::new(&note_ports_config).with_sample_offset_range(-4..=64);
    let mut audio_buffers = if in_place {
        AudioBuffers::new_in_place_f64(&audio_ports_config, BUFFER_SIZE)?
    } else {
        AudioBuffers::new_out_of_place_f64(&audio_ports_config, BUFFER_SIZE)
    };

    plugin.on_audio_thread(|plugin| -> Result<()> {
        let mut process = ProcessScope::new(&plugin, &mut audio_buffers)?;

        for _ in 0..5 {
            plugin.poll_callback();
            process.audio_buffers().fill_white_noise(&mut prng);
            process.add_events(note_rng.generate_events(&mut prng, BUFFER_SIZE));
            process.run()?;
        }

        Ok(())
    })?;

    plugin.poll_callback(|_| Ok(()))?;

    Ok(TestStatus::Success { details: None })
}

/// The test for `PluginTestCase::ProcessAudioDenormal`.
pub fn test_process_audio_denormals(library: &PluginLibrary, plugin_id: &str) -> Result<TestStatus> {
    let mut prng = new_prng();

    let plugin = library
        .create_plugin(plugin_id)
        .context("Could not create the plugin instance")?;
    plugin.init().context("Error during initialization")?;

    let audio_ports_config = match plugin.get_extension::<AudioPorts>() {
        Some(audio_ports) => audio_ports
            .config()
            .context("Error while querying 'audio-ports' IO configuration")?,
        None => {
            return Ok(TestStatus::Skipped {
                details: Some(String::from(
                    "The plugin does not implement the 'audio-ports' extension.",
                )),
            });
        }
    };

    let note_ports_config = match plugin.get_extension::<NotePorts>() {
        Some(note_ports) => note_ports
            .config()
            .context("Error while querying 'note-ports' IO configuration")?,
        None => NotePortConfig::default(),
    };

    if audio_ports_config.inputs.is_empty() {
        return Ok(TestStatus::Skipped {
            details: Some(String::from(
                "The plugin implements the 'audio-ports' extension but it does not have any input audio ports.",
            )),
        });
    }

    let mut audio_buffers = AudioBuffers::new_out_of_place_f32(&audio_ports_config, BUFFER_SIZE);
    let mut note_rng = NoteGenerator::new(&note_ports_config).with_sample_offset_range(-4..=128);

    let time_normal = Instant::now();
    plugin.on_audio_thread(|plugin| -> Result<()> {
        let mut process = ProcessScope::new(&plugin, &mut audio_buffers)?;
        process.set_allow_denormals(true);

        for _ in 0..50 {
            plugin.poll_callback();
            process.add_events(note_rng.generate_events(&mut prng, BUFFER_SIZE));
            process.audio_buffers().fill_white_noise(&mut prng);
            process.run()?;
        }

        Ok(())
    })?;
    let time_normal = time_normal.elapsed();

    plugin.poll_callback(|_| Ok(()))?;

    let time_denormal = Instant::now();
    plugin.on_audio_thread(|plugin| -> Result<()> {
        let mut process = ProcessScope::new(&plugin, &mut audio_buffers)?;
        process.set_allow_denormals(true);

        for _ in 0..50 {
            for buffer in process.audio_buffers().iter_mut() {
                if buffer.port().input().is_some() {
                    buffer.set_input_constant_mask(ConstantMask::DYNAMIC);
                    for channel in 0..buffer.channels() {
                        match buffer.channel_mut(channel) {
                            Either::Left(c) => c.fill_with(|| prng.random_range(-f32::MIN_POSITIVE..f32::MIN_POSITIVE)),
                            Either::Right(c) => {
                                c.fill_with(|| prng.random_range(-f64::MIN_POSITIVE..f64::MIN_POSITIVE))
                            }
                        }
                    }
                }
            }

            plugin.poll_callback();
            process.add_events(note_rng.generate_events(&mut prng, BUFFER_SIZE));
            process.run()?;
        }

        Ok(())
    })?;
    let time_denormal = time_denormal.elapsed();

    plugin.poll_callback(|_| Ok(()))?;

    let ratio = time_denormal.as_secs_f64() / time_normal.as_secs_f64();
    if ratio > 2.0 {
        return Ok(TestStatus::Warning {
            details: Some(format!(
                "The plugin took {:.2}x longer to process denormals, you should set flush-to-zero flags or avoid \
                 denormals in some other way.",
                ratio
            )),
        });
    }

    if ratio > 1.2 {
        return Ok(TestStatus::Success {
            details: Some(format!("The plugin took {:.2}x longer to process denormals", ratio)),
        });
    }

    Ok(TestStatus::Success { details: None })
}

/// The test for `PluginTestCase::ProcessNoteOutOfPlaceBasic` and `PluginTestCase::ProcessNoteInconsistent`. This test is very similar to
/// `ProcessAudioOutOfPlaceBasic`, but it requires the `note-ports` extension, sends notes and/or
/// MIDI to the plugin, and doesn't require the `audio-ports` extension.
pub fn test_process_note_out_of_place(
    library: &PluginLibrary,
    plugin_id: &str,
    inconsistent: bool,
    wildcard: bool,
) -> Result<TestStatus> {
    let mut prng = new_prng();

    let plugin = library
        .create_plugin(plugin_id)
        .context("Could not create the plugin instance")?;
    plugin.init().context("Error during initialization")?;

    // You can have note/MIDI-only plugins, so not having any audio ports is perfectly fine here
    let audio_ports_config = match plugin.get_extension::<AudioPorts>() {
        Some(audio_ports) => audio_ports
            .config()
            .context("Error while querying 'audio-ports' IO configuration")?,
        None => AudioPortConfig::default(),
    };

    let note_ports_config = match plugin.get_extension::<NotePorts>() {
        Some(note_ports) => note_ports
            .config()
            .context("Error while querying 'note-ports' IO configuration")?,
        None => {
            return Ok(TestStatus::Skipped {
                details: Some(String::from(
                    "The plugin does not implement the 'note-ports' extension.",
                )),
            });
        }
    };

    if note_ports_config.inputs.is_empty() {
        return Ok(TestStatus::Skipped {
            details: Some(String::from(
                "The plugin implements the 'note-ports' extension but it does not have any input note ports.",
            )),
        });
    }

    if wildcard && !note_ports_config.inputs.iter().any(|x| x.supports_clap()) {
        return Ok(TestStatus::Skipped {
            details: Some(String::from(
                "The plugin does not have any input note ports that support CLAP events",
            )),
        });
    }

    plugin.on_audio_thread(|plugin| -> Result<()> {
        // We'll fill the input event queue with (consistent) random CLAP note and/or MIDI
        // events depending on what's supported by the plugin supports
        let mut audio_buffers = AudioBuffers::new_out_of_place_f32(&audio_ports_config, BUFFER_SIZE);
        let mut note_rng = NoteGenerator::new(&note_ports_config);
        let mut process = ProcessScope::new(&plugin, &mut audio_buffers)?;

        // voice_info::get needs to be called in an active state
        process.activate()?;

        let supports_overlapping_notes = plugin.on_main_thread(|plugin| {
            plugin
                .get_extension::<VoiceInfo>()
                .and_then(|x| x.get())
                .is_some_and(|info| (info.flags & CLAP_VOICE_INFO_SUPPORTS_OVERLAPPING_NOTES) != 0)
        });

        if inconsistent {
            note_rng = note_rng.with_inconsistent_events();
        }

        if supports_overlapping_notes {
            note_rng = note_rng.with_overlapping_notes();
        }

        if wildcard {
            note_rng = note_rng.with_wildcard_events();
        }

        for _ in 0..5 {
            plugin.poll_callback();
            process.audio_buffers().fill_white_noise(&mut prng);
            process.add_events(note_rng.generate_events(&mut prng, BUFFER_SIZE));
            process.run()?;
        }

        Ok(())
    })?;

    plugin.poll_callback(|_| Ok(()))?;

    Ok(TestStatus::Success { details: None })
}

/// The test for `PluginTestCase::ProcessVaryingSampleRates`.
pub fn test_process_varying_sample_rates(library: &PluginLibrary, plugin_id: &str) -> Result<TestStatus> {
    const SAMPLE_RATES: &[f64] = &[
        8000.0, 22050.0, 44100.0, 48000.0, 88200.0, 96000.0, 192000.0, 384000.0, 768000.0, 1234.5678, 12345.678,
        45678.901, 123456.78,
    ];

    let mut prng = new_prng();

    let plugin = library
        .create_plugin(plugin_id)
        .context("Could not create the plugin instance")?;
    plugin.init().context("Error during initialization")?;

    let audio_ports_config = plugin
        .get_extension::<AudioPorts>()
        .map(|x| x.config())
        .transpose()
        .context("Error while querying 'audio-ports' IO configuration")?
        .unwrap_or_default();

    let note_ports_config = plugin
        .get_extension::<NotePorts>()
        .map(|x| x.config())
        .transpose()
        .context("Error while querying 'note-ports' IO configuration")?
        .unwrap_or_default();

    let mut audio_buffers = AudioBuffers::new_out_of_place_f32(&audio_ports_config, BUFFER_SIZE);

    for &sample_rate in SAMPLE_RATES {
        let _span = Span::begin("SampleRate", record! { sample_rate: sample_rate });

        plugin
            .on_audio_thread(|plugin| -> Result<()> {
                let mut note_rng = NoteGenerator::new(&note_ports_config).with_sample_offset_range(-4..=64);
                let mut process = ProcessScope::with_config(&plugin, &mut audio_buffers, sample_rate, 1)?;

                for _ in 0..5 {
                    plugin.poll_callback();
                    process.audio_buffers().fill_white_noise(&mut prng);
                    process.add_events(note_rng.generate_events(&mut prng, BUFFER_SIZE));
                    process.run()?;
                }

                Ok(())
            })
            .with_context(|| format!("Error while processing with {:.2}hz sample rate", sample_rate))?;
    }

    plugin.poll_callback(|_| Ok(()))?;

    Ok(TestStatus::Success { details: None })
}

/// The test for `PluginTestCase::ProcessVaryingBlockSizes`.
pub fn test_process_varying_block_sizes(library: &PluginLibrary, plugin_id: &str) -> Result<TestStatus> {
    const BLOCK_SIZES: &[u32] = &[1, 256, 1024, 4096, 16384, 1536, 10, 17, 2027];

    let mut prng = new_prng();

    let plugin = library
        .create_plugin(plugin_id)
        .context("Could not create the plugin instance")?;
    plugin.init().context("Error during initialization")?;

    let audio_ports_config = plugin
        .get_extension::<AudioPorts>()
        .map(|x| x.config())
        .transpose()
        .context("Error while querying 'audio-ports' IO configuration")?
        .unwrap_or_default();

    let note_ports_config = plugin
        .get_extension::<NotePorts>()
        .map(|x| x.config())
        .transpose()
        .context("Error while querying 'note-ports' IO configuration")?
        .unwrap_or_default();

    for &buffer_size in BLOCK_SIZES {
        let _span = Span::begin("BlockSize", record! { buffer_size: buffer_size });

        plugin
            .on_audio_thread(|plugin| -> Result<()> {
                let mut audio_buffers = AudioBuffers::new_out_of_place_f32(&audio_ports_config, buffer_size);
                let mut note_rng = NoteGenerator::new(&note_ports_config).with_sample_offset_range(-4..=64);
                let mut process = ProcessScope::new(&plugin, &mut audio_buffers)?;
                let num_iters = (16384 / buffer_size).min(5);

                for _ in 0..num_iters {
                    plugin.poll_callback();
                    process.audio_buffers().fill_white_noise(&mut prng);
                    process.add_events(note_rng.generate_events(&mut prng, buffer_size));
                    process.run()?;
                }

                Ok(())
            })
            .with_context(|| format!("Error while processing with buffer size of {}", buffer_size))?;
    }

    plugin.poll_callback(|_| Ok(()))?;

    Ok(TestStatus::Success { details: None })
}

/// The test for `PluginTestCase::ProcessRandomBlockSizes`.
pub fn test_process_random_block_sizes(library: &PluginLibrary, plugin_id: &str) -> Result<TestStatus> {
    const MAX_BUFFER_SIZE: u32 = 2048;

    let mut prng = new_prng();

    let plugin = library
        .create_plugin(plugin_id)
        .context("Could not create the plugin instance")?;
    plugin.init().context("Error during initialization")?;

    let audio_ports_config = plugin
        .get_extension::<AudioPorts>()
        .map(|x| x.config())
        .transpose()
        .context("Error while querying 'audio-ports' IO configuration")?
        .unwrap_or_default();

    let note_ports_config = plugin
        .get_extension::<NotePorts>()
        .map(|x| x.config())
        .transpose()
        .context("Error while querying 'note-ports' IO configuration")?
        .unwrap_or_default();

    plugin.on_audio_thread(|plugin| -> Result<()> {
        let mut audio_buffers = AudioBuffers::new_out_of_place_f32(&audio_ports_config, MAX_BUFFER_SIZE);
        let mut note_rng = NoteGenerator::new(&note_ports_config).with_sample_offset_range(-4..=64);
        let mut process = ProcessScope::new(&plugin, &mut audio_buffers)?;

        for _ in 0..20 {
            let block_size = if prng.random_bool(0.8) {
                prng.random_range(2..=MAX_BUFFER_SIZE)
            } else {
                1
            };

            plugin.poll_callback();
            process.audio_buffers().fill_white_noise(&mut prng);
            process.add_events(note_rng.generate_events(&mut prng, block_size));
            process
                .run_with(block_size)
                .with_context(|| format!("Error while processing with buffer size of {}", block_size))?;
        }

        Ok(())
    })?;

    plugin.poll_callback(|_| Ok(()))?;

    Ok(TestStatus::Success { details: None })
}

/// The test for `PluginTestCase::ProcessSleepConstantMask`.
pub fn test_process_sleep_constant_mask(library: &PluginLibrary, plugin_id: &str) -> Result<TestStatus> {
    let mut prng = new_prng();

    let plugin = library
        .create_plugin(plugin_id)
        .context("Could not create the plugin instance")?;
    plugin.init().context("Error during initialization")?;

    let audio_ports_config = match plugin.get_extension::<AudioPorts>() {
        Some(audio_ports) => audio_ports
            .config()
            .context("Error while querying 'audio-ports' IO configuration")?,
        None => AudioPortConfig::default(),
    };

    let note_ports_config = match plugin.get_extension::<NotePorts>() {
        Some(note_ports) => note_ports
            .config()
            .context("Error while querying 'note-ports' IO configuration")?,
        None => NotePortConfig::default(),
    };

    let mut has_received_constant_output = false;
    let mut has_received_constant_flag = false;
    let mut check_buffers = |buffers: &AudioBuffers| -> Result<()> {
        for buffer in buffers.iter() {
            if buffer.port().output().is_none() {
                continue;
            }

            for channel in 0..buffer.channels() {
                let is_constant = check_channel_quiet(buffer.channel(channel), true);
                let marked_constant = buffer.get_output_constant_mask().is_channel_constant(channel);

                // congruency of these two is checked in [`ProcessScope::run`]

                if marked_constant {
                    has_received_constant_flag |= true;
                }

                if is_constant.is_ok() {
                    has_received_constant_output |= true;
                }
            }
        }

        Ok(())
    };

    plugin.on_audio_thread(|plugin| -> Result<()> {
        let mut audio_buffers = AudioBuffers::new_out_of_place_f32(&audio_ports_config, BUFFER_SIZE);
        let mut note_rng = NoteGenerator::new(&note_ports_config).with_sample_offset_range(-4..=64);
        let mut process = ProcessScope::new(&plugin, &mut audio_buffers)?;

        // block 1: silent inputs, see what the plugin does
        let span = Span::begin("BlockPrerollSilent", ());
        process.run()?;
        check_buffers(process.audio_buffers()).context("Block preroll silent")?;
        span.finish(());

        plugin.poll_callback();

        // block 2: randomize inputs, see if the plugin tracks constant channels
        let span = Span::begin("BlockActiveInput", ());
        process.audio_buffers().fill_white_noise(&mut prng);
        process.add_events(note_rng.generate_events(&mut prng, BUFFER_SIZE));
        process.run()?;
        check_buffers(process.audio_buffers()).context("Block random input")?;
        span.finish(());

        plugin.poll_callback();

        // block 3-40: silent inputs again, see if the plugin updates the constant mask accordingly
        // 40 blocks to give the output tail to fully decay to silence if there is any reverb/delay
        let span = Span::begin("BlockTailSilent", ());
        process.audio_buffers().fill_silence();
        process.add_events(note_rng.stop_all_voices(0));
        for _ in 3..=40 {
            process.run()?;
            check_buffers(process.audio_buffers())?;
        }
        span.finish(());

        Ok(())
    })?;

    plugin.poll_callback(|_| Ok(()))?;

    if !has_received_constant_flag && has_received_constant_output {
        return Ok(TestStatus::Success {
            details: Some(String::from("The plugin never set the constant flag on any output")),
        });
    }

    Ok(TestStatus::Success { details: None })
}

/// The test for `PluginTestCase::ProcessSleepProcessStatus`.
pub fn test_process_sleep_process_status(library: &PluginLibrary, plugin_id: &str) -> Result<TestStatus> {
    let mut prng = new_prng();

    let plugin = library
        .create_plugin(plugin_id)
        .context("Could not create the plugin instance")?;
    plugin.init().context("Error during initialization")?;

    let audio_ports_config = match plugin.get_extension::<AudioPorts>() {
        Some(audio_ports) => audio_ports
            .config()
            .context("Error while querying 'audio-ports' IO configuration")?,
        None => AudioPortConfig::default(),
    };

    let note_ports_config = match plugin.get_extension::<NotePorts>() {
        Some(note_ports) => note_ports
            .config()
            .context("Error while querying 'note-ports' IO configuration")?,
        None => NotePortConfig::default(),
    };

    let mut has_ever_slept = false;

    plugin.on_audio_thread(|plugin| -> Result<()> {
        let tail = plugin.get_extension::<Tail>();

        let mut audio_buffers = AudioBuffers::new_out_of_place_f32(&audio_ports_config, BUFFER_SIZE);
        let mut note_rng = NoteGenerator::new(&note_ports_config).with_sample_offset_range(-4..=64);
        let mut process = ProcessScope::new(&plugin, &mut audio_buffers)?;

        let mut is_sleeping = false;
        let mut quiet_time = 0;

        for is_quiet in [true, false, true, false, true, true] {
            let _span = if is_quiet {
                Span::begin("BlockQuiet", ())
            } else {
                Span::begin("BlockActive", ())
            };

            for _ in 0..10 {
                if is_quiet {
                    process.add_events(note_rng.stop_all_voices(0));
                    process.audio_buffers().fill_silence();
                } else {
                    process.add_events(note_rng.generate_events(&mut prng, BUFFER_SIZE));
                    process.audio_buffers().fill_white_noise(&mut prng);
                }

                plugin.poll_callback_with(|_, event| match event {
                    CallbackEvent::RequestProcess => {
                        is_sleeping = false;
                        Ok(())
                    }

                    _ => Ok(()),
                })?;

                let status = process.run()?;

                if is_sleeping && is_quiet {
                    for buffer in process.audio_buffers().iter() {
                        let Some(output) = buffer.port().output() else {
                            continue;
                        };

                        for channel in 0..buffer.channels() {
                            let is_constant = check_channel_quiet(buffer.channel(channel), true);
                            if let Err(db) = is_constant {
                                anyhow::bail!(
                                    "The plugin is sleeping but output port {output}, channel {channel} contains \
                                     non-constant data ({db:.2} dBFS)",
                                );
                            }
                        }
                    }
                }

                has_ever_slept |= is_sleeping;

                match status {
                    ProcessStatus::Continue => is_sleeping = false,
                    ProcessStatus::Sleep => is_sleeping = true,
                    ProcessStatus::ContinueIfNotQuiet => {
                        let is_output_quiet = process
                            .audio_buffers()
                            .iter()
                            .filter(|b| b.port().output().is_some())
                            .all(|b| b.get_output_constant_mask().are_all_channels_constant(b.channels()));

                        is_sleeping = is_output_quiet;
                    }

                    ProcessStatus::Tail => {
                        let tail = match &tail {
                            Some(tail) => tail.get(),
                            None => {
                                anyhow::bail!(
                                    "Plugin returned `CLAP_PROCESS_TAIL` process status but does not implement the \
                                     'tail' extension."
                                );
                            }
                        };

                        is_sleeping = tail < quiet_time;
                        if is_quiet {
                            quiet_time += BUFFER_SIZE;
                        } else {
                            quiet_time = 0;
                        }
                    }
                }
            }
        }

        Ok(())
    })?;

    plugin.poll_callback(|_| Ok(()))?;

    if !has_ever_slept {
        return Ok(TestStatus::Success {
            details: Some(String::from("The plugin never went to sleep during the test.")),
        });
    }

    Ok(TestStatus::Success { details: None })
}

/// The test for `PluginTestCase::ProcessResetReactivate`.
pub fn test_process_reset_reactivate(library: &PluginLibrary, plugin_id: &str) -> Result<TestStatus> {
    let mut prng = new_prng();
    let plugin = library
        .create_plugin(plugin_id)
        .context("Could not create the plugin instance")?;
    plugin.init().context("Error during initialization")?;

    let audio_ports_config = plugin
        .get_extension::<AudioPorts>()
        .map(|x| x.config())
        .transpose()
        .context("Error while querying 'audio-ports' IO configuration")?
        .unwrap_or_default();

    let note_ports_config = plugin
        .get_extension::<NotePorts>()
        .map(|x| x.config())
        .transpose()
        .context("Error while querying 'note-ports' IO configuration")?
        .unwrap_or_default();

    let result = plugin.on_audio_thread(|plugin| -> Result<TestStatus> {
        let mut audio_buffers = AudioBuffers::new_out_of_place_f32(&audio_ports_config, BUFFER_SIZE);
        let mut note_rng = NoteGenerator::new(&note_ports_config).with_sample_offset_range(-4..=64);
        let mut process = ProcessScope::new(&plugin, &mut audio_buffers)?;

        // first run, the "control"
        let span = Span::begin("InitialRun", ());
        process.audio_buffers().fill_white_noise(&mut prng);
        process.add_events(note_rng.generate_events(&mut prng, BUFFER_SIZE));
        process.run()?;
        span.finish(());

        plugin.poll_callback();
        process.deactivate();
        note_rng.reset();

        // second run, deactivate and reactivate the plugin
        let span = Span::begin("ReactivateRun", ());
        process.audio_buffers().fill_white_noise(&mut prng);
        process.add_events(note_rng.generate_events(&mut prng, BUFFER_SIZE));
        process.run()?;
        span.finish(());

        plugin.poll_callback();
        process.reset();
        note_rng.reset();

        // third run, reset the plugin
        let span = Span::begin("ResetRun", ());
        process.audio_buffers().fill_white_noise(&mut new_prng());
        process.add_events(note_rng.generate_events(&mut new_prng(), BUFFER_SIZE));
        process.run()?;
        span.finish(());

        plugin.poll_callback();

        Ok(TestStatus::Success { details: None })
    })?;

    plugin.poll_callback(|_| Ok(()))?;

    Ok(result)
}
