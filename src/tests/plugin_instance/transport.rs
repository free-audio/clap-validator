use crate::cli::tracing::{Span, record};
use crate::plugin::ext::audio_ports::{AudioPortConfig, AudioPorts};
use crate::plugin::ext::note_ports::{NotePortConfig, NotePorts};
use crate::plugin::library::PluginLibrary;
use crate::plugin::process::{AudioBuffers, Event, ProcessScope, TransportState};
use crate::tests::TestStatus;
use crate::tests::rng::{NoteGenerator, TransportFuzzer, new_prng};
use anyhow::{Context, Result};

const BUFFER_SIZE: u32 = 128;

/// The test for `PluginTestCase::TransportNull`
pub fn test_transport_null(library: &PluginLibrary, plugin_id: &str) -> Result<TestStatus> {
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

    plugin.on_audio_thread(|plugin| -> Result<()> {
        let mut note_rng = NoteGenerator::new(&note_ports_config).with_sample_offset_range(-1..=128);
        let mut audio_buffers = AudioBuffers::new_out_of_place_f32(&audio_ports_config, BUFFER_SIZE);
        let mut process = ProcessScope::new(&plugin, &mut audio_buffers)?;

        for _ in 0..5 {
            process.transport().is_freerun = true;
            process.add_events(note_rng.generate_events(&mut prng, BUFFER_SIZE));
            process.audio_buffers().fill_white_noise(&mut prng);
            process.run()?;
        }

        Ok(())
    })?;

    plugin.poll_callback(|_| Ok(()))?;

    Ok(TestStatus::Success { details: None })
}

/// The test for `PluginTestCase::TransportFuzz`
pub fn test_transport_fuzz(library: &PluginLibrary, plugin_id: &str) -> Result<TestStatus> {
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

    plugin.on_audio_thread(|plugin| -> Result<()> {
        let mut transport_fuzz = TransportFuzzer::new();
        let mut note_rng = NoteGenerator::new(&note_ports_config).with_sample_offset_range(-1..=128);
        let mut audio_buffers = AudioBuffers::new_out_of_place_f32(&audio_ports_config, BUFFER_SIZE);
        let mut process = ProcessScope::new(&plugin, &mut audio_buffers)?;

        for _ in 0..80 {
            transport_fuzz.mutate(&mut prng, process.transport());

            process.add_events(note_rng.generate_events(&mut prng, BUFFER_SIZE));
            process.audio_buffers().fill_white_noise(&mut prng);
            process.run()?;
        }

        Ok(())
    })?;

    plugin.poll_callback(|_| Ok(()))?;

    Ok(TestStatus::Success { details: None })
}

/// The test for `PluginTestCase::TransportFuzzSampleAccurate`
pub fn test_transport_fuzz_sample_accurate(library: &PluginLibrary, plugin_id: &str) -> Result<TestStatus> {
    const INTERVALS: &[u32] = &[1000, 100, 1];

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

    for &interval in INTERVALS {
        let _span = Span::begin("Interval", record! { interval: interval });

        plugin
            .on_audio_thread(|plugin| -> Result<()> {
                let mut note_rng = NoteGenerator::new(&note_ports_config).with_sample_offset_range(-1..=128);
                let mut audio_buffers = AudioBuffers::new_out_of_place_f32(&audio_ports_config, BUFFER_SIZE);
                let mut process = ProcessScope::new(&plugin, &mut audio_buffers)?;

                let mut transport_fuzz = TransportFuzzer::new();
                let mut transport_state = TransportState::default();
                let mut transport_start = TransportState::default();
                let mut current_sample = 0;

                for _ in 0..20 {
                    // reset transport state at the start of each block
                    process.transport().clone_from(&transport_start);

                    // add sample-accurate transport events
                    while current_sample < BUFFER_SIZE {
                        // save transport state at the start of the next block
                        if current_sample + interval >= BUFFER_SIZE {
                            transport_start = transport_state.clone();
                            transport_start.advance((BUFFER_SIZE - current_sample) as i64, process.sample_rate());
                        }

                        // advance transport state to the event position, mutate it, and add the event
                        transport_state.advance(interval as i64, process.sample_rate());
                        transport_fuzz.mutate(&mut prng, &mut transport_state);

                        // this will also send the event at current_sample == 0
                        // but that's fine, the plugin should handle that correctly
                        process.add_events([Event::Transport(transport_state.as_clap_transport(current_sample))]);
                        current_sample += interval;
                    }

                    current_sample -= BUFFER_SIZE;

                    process.audio_buffers().fill_white_noise(&mut prng);
                    process.add_events(note_rng.generate_events(&mut prng, BUFFER_SIZE));
                    process.run()?;
                }

                Ok(())
            })
            .with_context(|| {
                format!(
                    "Error during sample-accurate transport test with interval of {} samples",
                    interval
                )
            })?;
    }

    plugin.poll_callback(|_| Ok(()))?;

    Ok(TestStatus::Success { details: None })
}
