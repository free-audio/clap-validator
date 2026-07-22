//! Contains all types and implementations related to parameter management.

use crate::{PolySynthAudioProcessor, PolySynthPluginMainThread};
use clack_extensions::params::*;
use clack_extensions::state::PluginStateImpl;
use clack_plugin::events::event_types::{ParamModEvent, ParamValueEvent};
use clack_plugin::events::spaces::CoreEventSpace;
use clack_plugin::prelude::*;
use clack_plugin::stream::{InputStream, OutputStream};
use std::ffi::CStr;
use std::fmt::Write as _;
use std::io::{Read, Write as _};
use std::sync::atomic::{AtomicU32, Ordering};

pub const PARAM_VOLUME_ID: ClapId = ClapId::new(1);

const DEFAULT_VOLUME: f32 = 0.2;

pub struct PolySynthParams {
    volume: AtomicF32,
}

impl PolySynthParams {
    pub fn new() -> Self {
        Self {
            volume: AtomicF32::new(DEFAULT_VOLUME),
        }
    }

    #[inline]
    pub fn get_volume(&self) -> f32 {
        self.volume.load(Ordering::SeqCst)
    }

    #[inline]
    pub fn set_volume(&self, new_volume: f32) {
        let new_volume = new_volume.clamp(0., 1.);
        self.volume.store(new_volume, Ordering::SeqCst)
    }

    pub fn handle_event(&self, event: &ParamValueEvent) {
        if event.param_id() == PARAM_VOLUME_ID {
            self.set_volume(event.value() as f32)
        }
    }
}

pub struct PolySynthParamModulations {
    volume_mod: f32,
}

impl PolySynthParamModulations {
    pub fn new() -> Self {
        Self { volume_mod: 0.0 }
    }

    #[inline]
    pub fn volume(&self) -> f32 {
        self.volume_mod
    }

    pub fn handle_event(&mut self, event: &ParamModEvent) {
        if event.param_id() == PARAM_VOLUME_ID {
            self.volume_mod = event.amount() as f32
        }
    }
}

impl PluginStateImpl for PolySynthPluginMainThread<'_> {
    fn save(&mut self, output: &mut OutputStream) -> Result<(), PluginError> {
        let volume_param = self.shared.params.get_volume();

        output.write_all(b"clck")?;
        output.write_all(&volume_param.to_le_bytes())?;
        Ok(())
    }

    fn load(&mut self, input: &mut InputStream) -> Result<(), PluginError> {
        let mut buf = [0; 4];
        input.read_exact(&mut buf)?;
        if buf != *b"clck" {
            return Err(PluginError::Message("invalid magic header"));
        }

        input.read_exact(&mut buf)?;
        let volume_value = f32::from_le_bytes(buf);
        self.shared.params.set_volume(volume_value);

        if let Some(ext) = self.host.get_extension::<HostParams>() {
            ext.rescan(&mut self.host, ParamRescanFlags::VALUES);
        }

        Ok(())
    }
}

impl PluginMainThreadParams for PolySynthPluginMainThread<'_> {
    fn count(&mut self) -> u32 {
        1
    }

    fn get_info(&mut self, param_index: u32, info: &mut ParamInfoWriter) {
        if param_index == 0 {
            info.set(&ParamInfo {
                id: PARAM_VOLUME_ID,
                flags: ParamInfoFlags::IS_AUTOMATABLE
                    | ParamInfoFlags::IS_MODULATABLE
                    | ParamInfoFlags::IS_AUTOMATABLE_PER_CHANNEL
                    | ParamInfoFlags::IS_AUTOMATABLE_PER_KEY
                    | ParamInfoFlags::IS_AUTOMATABLE_PER_NOTE_ID
                    | ParamInfoFlags::IS_MODULATABLE_PER_CHANNEL
                    | ParamInfoFlags::IS_MODULATABLE_PER_KEY
                    | ParamInfoFlags::IS_MODULATABLE_PER_NOTE_ID,
                cookie: Default::default(),
                name: b"Volume",
                module: b"",
                min_value: 0.0,
                max_value: 1.0,
                default_value: DEFAULT_VOLUME as f64,
            })
        }
    }

    fn get_value(&mut self, param_id: ClapId) -> Option<f64> {
        match param_id {
            PARAM_VOLUME_ID => Some(self.shared.params.get_volume() as f64),
            _ => None,
        }
    }

    fn value_to_text(&mut self, param_id: ClapId, value: f64, writer: &mut ParamDisplayWriter) -> std::fmt::Result {
        match param_id {
            PARAM_VOLUME_ID => write!(writer, "{0:.2} %", value * 100.0),
            _ => Err(std::fmt::Error),
        }
    }

    fn text_to_value(&mut self, param_id: ClapId, text: &CStr) -> Option<f64> {
        let text = text.to_str().ok()?;
        if param_id == PARAM_VOLUME_ID {
            let text = text.strip_suffix('%').unwrap_or(text).trim();
            let percentage: f64 = text.parse().ok()?;

            Some(percentage / 100.0)
        } else {
            None
        }
    }

    fn flush(&mut self, input_parameter_changes: &InputEvents, _output_parameter_changes: &mut OutputEvents) {
        for event in input_parameter_changes {
            if let Some(CoreEventSpace::ParamValue(event)) = event.as_core_event() {
                self.shared.params.handle_event(event)
            }
        }
    }
}

impl PluginAudioProcessorParams for PolySynthAudioProcessor<'_> {
    fn flush(&mut self, input_parameter_changes: &InputEvents, _output_parameter_changes: &mut OutputEvents) {
        for event in input_parameter_changes {
            self.handle_event(event)
        }
    }
}

/// A small helper to atomically load and store an `f32` value.
struct AtomicF32(AtomicU32);

impl AtomicF32 {
    #[inline]
    fn new(value: f32) -> Self {
        Self(AtomicU32::new(f32::to_bits(value)))
    }

    #[inline]
    fn store(&self, value: f32, order: Ordering) {
        self.0.store(f32::to_bits(value), order)
    }

    #[inline]
    fn load(&self, order: Ordering) -> f32 {
        f32::from_bits(self.0.load(order))
    }
}
