//! A wrapper around `clap_preset_discovery_provider`.

use anyhow::{Context, Result};
use std::ffi::CString;
use std::marker::PhantomData;
use std::pin::Pin;
use std::ptr::NonNull;

use clap_sys::factory::draft::preset_discovery::clap_preset_discovery_provider;

use super::indexer::{Indexer, IndexerResults};
use super::{PresetDiscoveryFactory, ProviderMetadata};
use crate::util::unsafe_clap_call;

/// A preset discovery provider created from a preset discovery factory. The provider is initialized
/// and the declared contents are read when the object is created, and the provider is destroyed
/// when this object is dropped.
#[derive(Debug)]
pub struct Provider<'a> {
    handle: NonNull<clap_preset_discovery_provider>,

    /// The data declared by the provider during the `init()` call.
    declared_data: IndexerResults,

    /// The indexer passed to the instance. This provides a callback interface for the plugin to
    /// declare locations, file types, and sound packs. This information can then be used to crawl
    /// the filesystem for preset files, which can finally be queried for information using the
    /// `clap_preset_discovery_provider::get_metadata()` function. A single preset file may contain
    /// multiple presets, and the plugin may also store internal presets.
    ///
    /// Since there are currently no extensions the plugin shouldn't be interacting with it anymore
    /// after the `init()` call, but it still needs outlive the provider.
    _indexer: Pin<Box<Indexer>>,
    /// The factory this provider was created form. Only used for the lifetime.
    _factory: &'a PresetDiscoveryFactory<'a>,
    /// To honor CLAP's thread safety guidelines, this provider cannot be shared with or sent to
    /// other threads.
    _send_sync_marker: PhantomData<*const ()>,
}

impl<'a> Provider<'a> {
    /// Create a wrapper around a preset discovery factory instance returned from a CLAP plugin's
    /// entry point.
    pub fn new(factory: &'a PresetDiscoveryFactory, provider_id: &str) -> Result<Self> {
        let indexer = Indexer::new();

        let provider_id_cstring =
            CString::new(provider_id).expect("The provider ID contained internal null bytes");
        let provider = {
            let factory = factory.as_ptr();
            let provider = unsafe_clap_call! {
                factory=>create(
                    factory,
                    indexer.clap_preset_discovery_indexer_ptr(),
                    provider_id_cstring.as_ptr()
                )
            };
            match NonNull::new(provider as *mut clap_preset_discovery_provider) {
                Some(provider) => provider,
                None => anyhow::bail!(
                    "'clap_preset_discovery_factory::create()' returned a null pointer for the \
                     provider with ID '{provider_id}'.",
                ),
            }
        };

        let declared_data = {
            let provider = provider.as_ptr();
            if !unsafe_clap_call! { provider=>init(provider) } {
                anyhow::bail!(
                    "'clap_preset_discovery_factory::init()' returned false for the provider with \
                     ID '{provider_id}'."
                );
            }

            // TODO: After this point the provider should not declare any more data. We don't
            //       currently test for this.
            indexer.results().with_context(|| {
                format!(
                    "Errors produced during 'clap_preset_discovery_indexer' callbacks made by the \
                     provider with ID '{provider_id}'"
                )
            })?
        };

        Ok(Provider {
            handle: provider,

            declared_data,

            _indexer: indexer,
            _factory: factory,
            _send_sync_marker: PhantomData,
        })
    }

    /// Get this provider's metadata descriptor. In theory this should be the same as the one
    /// retrieved from the factory earlier.
    pub fn descriptor(&self) -> Result<ProviderMetadata> {
        let provider = self.as_ptr();
        let descriptor = unsafe { (*provider).desc };
        if descriptor.is_null() {
            anyhow::bail!(
                "The 'desc' field on the 'clap_preset_provider' struct is a null pointer"
            );
        }

        ProviderMetadata::from_descriptor(unsafe { &*descriptor })
    }

    /// Get the data declared by the provider during its initialization.
    pub fn declared_data(&self) -> &IndexerResults {
        &self.declared_data
    }

    /// Get the raw pointer to the `clap_preset_discovery_provider` instance.
    pub fn as_ptr(&self) -> *const clap_preset_discovery_provider {
        self.handle.as_ptr()
    }
}
