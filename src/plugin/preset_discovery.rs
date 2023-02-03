//! An abstraction for the preset discovery factory.

use anyhow::{Context, Result};
use clap_sys::factory::draft::preset_discovery::{
    clap_preset_discovery_factory, clap_preset_discovery_provider_descriptor,
};
use clap_sys::version::{clap_version, clap_version_is_compatible};
use std::collections::HashSet;
use std::ptr::NonNull;

use super::library::PluginLibrary;
use crate::util::{self, unsafe_clap_call};

mod indexer;
mod provider;

pub use self::indexer::{FileType, IndexerResults, Location, LocationUri, Soundpack};
pub use self::provider::Provider;

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

/// Metadata (descriptor) for a preset discovery provider. These providers can be instantiated by
/// passing the IDs to [`PresetDiscoveryFactory::create()`].
#[derive(Debug, PartialEq, Eq)]
pub struct ProviderMetadata {
    pub version: (u32, u32, u32),
    pub id: String,
    pub name: String,
    pub vendor: String,
}

impl ProviderMetadata {
    /// Parse the metadata from a `clap_preset_discovery_provider_descriptor`.
    pub fn from_descriptor(descriptor: &clap_preset_discovery_provider_descriptor) -> Result<Self> {
        let metadata = ProviderMetadata {
            version: (
                descriptor.clap_version.major,
                descriptor.clap_version.minor,
                descriptor.clap_version.revision,
            ),
            id: unsafe { util::cstr_ptr_to_string(descriptor.id)? }
                .context("The provider's 'id' pointer was null")?,
            name: unsafe { util::cstr_ptr_to_string(descriptor.name)? }
                .context("The provider's 'name' pointer was null")?,
            vendor: unsafe { util::cstr_ptr_to_string(descriptor.vendor)? }
                .context("The provider's 'vendor' pointer was null")?,
        };

        if metadata.name.is_empty() {
            anyhow::bail!("The plugin declared a preset provider with an empty name.")
        }

        Ok(metadata)
    }

    /// Get the CLAP version representation for this provider.
    pub fn clap_version(&self) -> clap_version {
        clap_version {
            major: self.version.0,
            minor: self.version.1,
            revision: self.version.2,
        }
    }
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

    /// Return metadata for all of the preset discovery factory's providers. These providers can be
    /// instantiated for crawling and for retrieving more metadata using
    /// [`create()`][Self::create()].
    pub fn metadata(&self) -> Result<Vec<ProviderMetadata>> {
        let factory = self.as_ptr();
        let num_providers = unsafe_clap_call! { factory=>count(factory) };

        let mut metadata = Vec::with_capacity(num_providers as usize);
        for i in 0..num_providers {
            let descriptor = unsafe_clap_call! { factory=>get_descriptor(factory, i) };
            if descriptor.is_null() {
                anyhow::bail!(
                    "The preset discovery factory returned a null pointer for the descriptor at \
                     index {i} (expected {num_providers} total providers)."
                );
            }

            metadata.push(ProviderMetadata::from_descriptor(unsafe { &*descriptor })?);
        }

        // As a sanity check we'll make sure there are no duplicate IDs in here
        let unique_ids: HashSet<&str> = metadata
            .iter()
            .map(|provider_metadata| provider_metadata.id.as_str())
            .collect();
        if unique_ids.len() != metadata.len() {
            anyhow::bail!(
                "The preset discovery factory contains multiple entries for the same provider ID."
            );
        }

        Ok(metadata)
    }

    /// Create a preset provider based on one of the provider IDs returned by
    /// [`metadata()`][Self::metadata()].
    ///
    /// Returns an error if the provider's CLAP version is not supported.
    pub fn create_provider(&self, metadata: &ProviderMetadata) -> Result<Provider> {
        if !clap_version_is_compatible(metadata.clap_version()) {
            anyhow::bail!(
                "The preset provider with ID '{}' has an unsupported CLAP version {:?}",
                metadata.id,
                metadata.clap_version()
            );
        }

        Provider::new(self, &metadata.id)
    }
}
