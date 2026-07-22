// TODO: we should probably add a negative test (a plugin that fails some of the clap-validator tests)

use crate::params::{PolySynthParamModulations, PolySynthParams};
use crate::poly_oscillator::PolyOscillator;
use clack_extensions::audio_ports::*;
use clack_extensions::audio_ports_activation::{
    PluginAudioPortsActivation, PluginAudioPortsActivationImpl, PluginAudioPortsActivationSetImpl, SampleSize,
};
use clack_extensions::audio_ports_config::{
    AudioPortConfigWriter, AudioPortsConfiguration, MainPortInfo, PluginAudioPortsConfig, PluginAudioPortsConfigImpl,
    PluginAudioPortsConfigInfo, PluginAudioPortsConfigInfoImpl,
};
use clack_extensions::configurable_audio_ports::{
    AudioPortRequest, PluginConfigurableAudioPorts, PluginConfigurableAudioPortsImpl,
};
use clack_extensions::note_ports::*;
use clack_extensions::params::*;
use clack_extensions::state::PluginState;
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::prelude::*;
use clack_plugin::process::ConstantMask;
use std::f32;
use std::ffi::CString;

mod oscillator;
mod params;
mod poly_oscillator;

pub struct PolySynthPlugin;

impl Plugin for PolySynthPlugin {
    type AudioProcessor<'a> = PolySynthAudioProcessor<'a>;
    type Shared<'a> = PolySynthPluginShared;
    type MainThread<'a> = PolySynthPluginMainThread<'a>;

    fn declare_extensions(builder: &mut PluginExtensions<Self>, _shared: Option<&PolySynthPluginShared>) {
        builder
            .register::<PluginAudioPorts>()
            .register::<PluginNotePorts>()
            .register::<PluginParams>()
            .register::<PluginState>()
            .register::<PluginAudioPortsConfig>()
            .register::<PluginAudioPortsConfigInfo>()
            .register::<PluginAudioPortsActivation>()
            .register::<PluginConfigurableAudioPorts>();
    }
}

impl DefaultPluginFactory for PolySynthPlugin {
    fn get_descriptor() -> PluginDescriptor {
        use clack_plugin::plugin::features::*;

        PluginDescriptor::new("org.rust-audio.clack.polysynth", "Clack PolySynth Example").with_features([
            SYNTHESIZER,
            MONO,
            INSTRUMENT,
        ])
    }

    fn new_shared(_host: HostSharedHandle) -> Result<PolySynthPluginShared, PluginError> {
        Ok(PolySynthPluginShared {
            params: PolySynthParams::new(),
        })
    }

    fn new_main_thread<'a>(
        host: HostMainThreadHandle<'a>,
        shared: &'a PolySynthPluginShared,
    ) -> Result<PolySynthPluginMainThread<'a>, PluginError> {
        Ok(PolySynthPluginMainThread {
            host,
            shared,
            config: ClapId::new(2),
            active: true,
        })
    }
}

pub struct PolySynthAudioProcessor<'a> {
    channels: u32,
    active: bool,

    poly_osc: PolyOscillator,
    modulation_values: PolySynthParamModulations,
    shared: &'a PolySynthPluginShared,
}

impl<'a> PluginAudioProcessor<'a, PolySynthPluginShared, PolySynthPluginMainThread<'a>>
    for PolySynthAudioProcessor<'a>
{
    fn activate(
        _host: HostAudioProcessorHandle<'a>,
        main_thread: &mut PolySynthPluginMainThread,
        shared: &'a PolySynthPluginShared,
        audio_config: PluginAudioConfiguration,
    ) -> Result<Self, PluginError> {
        Ok(Self {
            active: main_thread.active,
            channels: main_thread.config.get(),
            poly_osc: PolyOscillator::new(16, audio_config.sample_rate as f32),
            modulation_values: PolySynthParamModulations::new(),
            shared,
        })
    }

    fn process(&mut self, _process: Process, mut audio: Audio, events: Events) -> Result<ProcessStatus, PluginError> {
        let mut output_port = audio
            .output_port(0)
            .ok_or(PluginError::Message("No output port found"))?;

        let mut output_channels = output_port
            .channels()?
            .into_f32()
            .ok_or(PluginError::Message("Expected f32 output"))?;

        let output_buffer = output_channels
            .channel_mut(0)
            .ok_or(PluginError::Message("Expected at least one channel"))?;

        output_buffer.fill(0.0);

        let mut is_non_silent = false;
        for event_batch in events.input.batch() {
            for event in event_batch.events() {
                self.handle_event(event);
            }

            let output_buffer = &mut output_buffer[event_batch.sample_bounds()];
            self.poly_osc.generate_next_samples(
                output_buffer,
                self.shared.params.get_volume(),
                self.modulation_values.volume(),
            );

            is_non_silent |= self.poly_osc.has_active_voices()
        }

        // it is legal; when an output port is deactivated, the host must not use its contents
        if !self.active {
            output_buffer.fill(f32::NAN);
        }

        assert!(output_channels.channel_count() == self.channels);

        // Copy the first channel to all other channels for mono output
        if output_channels.channel_count() > 1 {
            let (first_channel, other_channels) = output_channels.split_at_mut(1);
            let first_channel = first_channel.channel(0).unwrap();

            for other_channel in other_channels {
                other_channel.copy_from_slice(first_channel)
            }
        }

        if !is_non_silent {
            audio
                .output_port(0)
                .unwrap()
                .set_constant_mask(ConstantMask::FULLY_CONSTANT);
        }

        if self.poly_osc.has_active_voices() {
            Ok(ProcessStatus::Continue)
        } else {
            Ok(ProcessStatus::Sleep)
        }
    }

    fn stop_processing(&mut self) {
        self.poly_osc.stop_all();
    }

    fn reset(&mut self) {
        self.poly_osc.stop_all();
    }
}

impl PolySynthAudioProcessor<'_> {
    fn handle_event(&mut self, event: &UnknownEvent) {
        match event.as_core_event() {
            Some(CoreEventSpace::NoteOn(event)) => self.poly_osc.handle_note_on(event),
            Some(CoreEventSpace::NoteOff(event)) => self.poly_osc.handle_note_off(event),
            Some(CoreEventSpace::ParamValue(event)) => {
                if event.pckn().matches_all() {
                    self.shared.params.handle_event(event)
                } else {
                    self.poly_osc.handle_param_value(event)
                }
            }
            Some(CoreEventSpace::ParamMod(event)) => {
                if event.pckn().matches_all() {
                    self.modulation_values.handle_event(event)
                } else {
                    self.poly_osc.handle_param_mod(event)
                }
            }
            _ => {}
        }
    }
}

impl PluginAudioPortsImpl for PolySynthPluginMainThread<'_> {
    fn count(&mut self, is_input: bool) -> u32 {
        if is_input { 0 } else { 1 }
    }

    fn get(&mut self, index: u32, is_input: bool, writer: &mut AudioPortInfoWriter) {
        PluginAudioPortsConfigInfoImpl::get(self, self.config, index, is_input, writer);
    }
}

impl PluginNotePortsImpl for PolySynthPluginMainThread<'_> {
    fn count(&mut self, is_input: bool) -> u32 {
        if is_input { 1 } else { 0 }
    }

    fn get(&mut self, index: u32, is_input: bool, writer: &mut NotePortInfoWriter) {
        if is_input && index == 0 {
            writer.set(&NotePortInfo {
                id: ClapId::new(1),
                name: b"main",
                preferred_dialect: Some(NoteDialect::Clap),
                supported_dialects: NoteDialects::CLAP,
            })
        }
    }
}

impl PluginAudioPortsConfigImpl for PolySynthPluginMainThread<'_> {
    fn count(&mut self) -> u32 {
        8
    }

    fn get(&mut self, index: u32, writer: &mut AudioPortConfigWriter) {
        let channels = index + 1;
        writer.write(&AudioPortsConfiguration {
            id: ClapId::new(channels),
            name: CString::new(format!("Config #{}", channels)).unwrap().as_bytes(),
            input_port_count: 0,
            output_port_count: 1,
            main_input: None,
            main_output: Some(MainPortInfo {
                channel_count: channels,
                port_type: AudioPortType::from_channel_count(channels),
            }),
        });
    }

    fn select(&mut self, config_id: ClapId) -> Result<(), PluginError> {
        if config_id.get() <= 8 {
            self.config = config_id;
            self.active = true;
            Ok(())
        } else {
            Err(PluginError::Message("Invalid configuration ID"))
        }
    }
}

impl PluginAudioPortsConfigInfoImpl for PolySynthPluginMainThread<'_> {
    fn current_config(&mut self) -> Option<ClapId> {
        Some(self.config)
    }

    fn get(&mut self, config_id: ClapId, index: u32, is_input: bool, writer: &mut AudioPortInfoWriter) {
        let channels = config_id.get();

        if !is_input && index == 0 {
            writer.set(&AudioPortInfo {
                id: ClapId::new(1),
                name: b"main",
                channel_count: channels,
                flags: AudioPortFlags::IS_MAIN,
                port_type: AudioPortType::from_channel_count(channels),
                in_place_pair: None,
            });
        }
    }
}

impl PluginConfigurableAudioPortsImpl for PolySynthPluginMainThread<'_> {
    fn can_apply_configuration(&mut self, requests: &[AudioPortRequest]) -> bool {
        matches!(requests.first(), Some(request) if !request.is_input() && request.port_index() == 0 && request.details().channel_count() > 0 && request.details().channel_count() <= 8)
    }

    fn apply_configuration(&mut self, requests: &[AudioPortRequest]) -> bool {
        match requests.first() {
            Some(request)
                if !request.is_input()
                    && request.port_index() == 0
                    && request.details().channel_count() > 0
                    && request.details().channel_count() <= 8 =>
            {
                self.config = ClapId::new(request.details().channel_count());
                true
            }
            _ => false,
        }
    }
}

impl PluginAudioPortsActivationImpl for PolySynthPluginMainThread<'_> {
    fn can_activate_while_processing(&mut self) -> bool {
        false
    }
}

impl PluginAudioPortsActivationSetImpl for PolySynthPluginMainThread<'_> {
    fn set_active(&mut self, is_input: bool, port_index: u32, is_active: bool, sample_size: SampleSize) -> bool {
        if is_input || port_index != 0 {
            return false;
        }

        if sample_size == SampleSize::Float64 {
            return false;
        }

        self.active = is_active;
        true
    }
}

impl PluginAudioPortsActivationSetImpl for PolySynthAudioProcessor<'_> {
    fn set_active(&mut self, _: bool, _: u32, _: bool, _: SampleSize) -> bool {
        false
    }
}

pub struct PolySynthPluginShared {
    params: PolySynthParams,
}

impl PluginShared<'_> for PolySynthPluginShared {}

pub struct PolySynthPluginMainThread<'a> {
    host: HostMainThreadHandle<'a>,
    shared: &'a PolySynthPluginShared,
    config: ClapId,
    active: bool,
}

impl<'a> PluginMainThread<'a, PolySynthPluginShared> for PolySynthPluginMainThread<'a> {}

clack_export_entry!(SinglePluginEntry<PolySynthPlugin>);
