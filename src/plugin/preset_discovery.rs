//! An abstraction for the preset discovery factory.

use anyhow::Result;
use clap_sys::factory::draft::preset_discovery::clap_preset_discovery_factory;
use clap_sys::factory::plugin_factory::clap_plugin_factory;
use clap_sys::plugin::clap_plugin;
use std::ffi::CStr;
use std::marker::PhantomData;
use std::ops::Deref;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::Arc;

use super::library::PluginLibrary;
use super::{assert_plugin_state_eq, assert_plugin_state_initialized};
use crate::util::unsafe_clap_call;

/// A `Send+Sync` wrapper around `*const clap_preset_discovery_factory`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct PresetDiscoveryHandle(pub NonNull<clap_preset_discovery_factory>);

unsafe impl Send for PresetDiscoveryHandle {}
unsafe impl Sync for PresetDiscoveryHandle {}

/// A wrapper around a CLAP preset discovery factory.
///
/// See <https://github.com/free-audio/clap/blob/main/include/clap/factory/draft/preset-discovery.h>
/// for more information.
#[derive(Debug)]
pub struct PresetDiscoveryFactory<'lib> {
    handle: PresetDiscoveryHandle,

    /// The CLAP plugin library this factory was created from. This field is not used directly, but
    /// keeping a reference to the library here prevents the factory from outliving the library.
    _library: &'lib PluginLibrary,
}

impl<'lib> PresetDiscoveryFactory<'lib> {
    /// Create a wrapper around a preset discovery factory instance returned from a CLAP plugin's
    /// entry point.
    pub fn new(
        library: &'lib PluginLibrary,
        factory: NonNull<clap_preset_discovery_factory>,
    ) -> Self {
        PresetDiscoveryFactory {
            handle: PresetDiscoveryHandle(factory),
            _library: library,
        }
    }

    /// Get the raw pointer to the `clap_preset_discovery_factory` instance.
    pub fn as_ptr(&self) -> *const clap_preset_discovery_factory {
        self.handle.0.as_ptr()
    }
}
