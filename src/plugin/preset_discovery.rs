//! An abstraction for the preset discovery factory.

use super::library::PluginLibrary;
use super::util::{self, clap_call};
use anyhow::{Context, Result};
use clap_sys::factory::preset_discovery::{clap_preset_discovery_factory, clap_preset_discovery_provider_descriptor};
use clap_sys::timestamp::{CLAP_TIMESTAMP_UNKNOWN, clap_timestamp};
use clap_sys::version::{clap_version, clap_version_is_compatible};
use std::collections::HashSet;
use std::ptr::NonNull;
use time::OffsetDateTime;

mod indexer;
mod metadata_receiver;
mod provider;

pub use self::indexer::{Flags, Location, LocationValue, Soundpack};
pub use self::metadata_receiver::{PluginAbi, Preset, PresetFile};
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
/// passing the metadata to [`PresetDiscoveryFactory::create_provider()`].
#[derive(Debug, PartialEq, Eq)]
pub struct ProviderMetadata {
    pub id: String,
    pub name: String,
    pub vendor: Option<String>,
    pub version: (u32, u32, u32),
}

impl ProviderMetadata {
    /// Parse the metadata from a `clap_preset_discovery_provider_descriptor`.
    pub unsafe fn from_descriptor(descriptor: *const clap_preset_discovery_provider_descriptor) -> Result<Self> {
        anyhow::ensure!(
            !descriptor.is_null(),
            "The preset discovery provider descriptor is a null pointer."
        );

        let descriptor = unsafe { &*descriptor };

        Ok(ProviderMetadata {
            id: unsafe { util::cstr_ptr_to_mandatory_string(descriptor.id) }
                .context("Error parsing the provider's 'id' field")?,
            name: unsafe { util::cstr_ptr_to_mandatory_string(descriptor.name) }
                .context("Error parsing the provider's 'name' field")?,
            vendor: unsafe { util::cstr_ptr_to_optional_string(descriptor.vendor) }
                .context("Error parsing the provider's 'vendor' field")?,
            version: (
                descriptor.clap_version.major,
                descriptor.clap_version.minor,
                descriptor.clap_version.revision,
            ),
        })
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
    pub fn new(library: &'lib PluginLibrary, factory: NonNull<clap_preset_discovery_factory>) -> Self {
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
        let num_providers = unsafe {
            clap_call! { factory=>count(factory) }
        };

        let mut metadata = Vec::with_capacity(num_providers as usize);
        for i in 0..num_providers {
            let descriptor = unsafe {
                clap_call! { factory=>get_descriptor(factory, i) }
            };

            if descriptor.is_null() {
                anyhow::bail!(
                    "The preset discovery factory returned a null pointer for the descriptor at index {i} (expected \
                     {num_providers} total providers)."
                );
            }

            metadata.push(unsafe { ProviderMetadata::from_descriptor(descriptor)? });
        }

        // As a sanity check we'll make sure there are no duplicate IDs in here
        let unique_ids: HashSet<&str> = metadata
            .iter()
            .map(|provider_metadata| provider_metadata.id.as_str())
            .collect();
        if unique_ids.len() != metadata.len() {
            anyhow::bail!("The preset discovery factory contains multiple entries for the same provider ID.");
        }

        Ok(metadata)
    }

    /// Create a preset provider based on one of the provider IDs returned by
    /// [`metadata()`][Self::metadata()].
    ///
    /// Returns an error if the provider's CLAP version is not supported.
    pub fn create_provider(&self, metadata: &ProviderMetadata) -> Result<Provider<'_>> {
        if !clap_version_is_compatible(metadata.clap_version()) {
            anyhow::bail!(
                "The preset provider with ID '{}' has an unsupported CLAP version {:?}.",
                metadata.id,
                metadata.clap_version()
            );
        }

        Provider::new(self, &metadata.id)
    }
}

/// Convert a `clap_timestamp` to an `Option<OffsetDateTime>`. A value of `CLAP_TIMESTAMP_UNKNOWN`
/// gets translated to `None`.
pub fn parse_timestamp(timestamp: clap_timestamp) -> Result<Option<OffsetDateTime>> {
    let parsed = if timestamp == CLAP_TIMESTAMP_UNKNOWN {
        None
    } else {
        Some(
            OffsetDateTime::from_unix_timestamp_nanos(timestamp as i128 * 1_000_000)
                .map_err(|_| anyhow::anyhow!("Could not parse the timestamp."))?,
        )
    };

    Ok(parsed)
}
