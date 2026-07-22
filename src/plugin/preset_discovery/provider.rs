//! A wrapper around `clap_preset_discovery_provider`.

use super::indexer::{Indexer, IndexerResults};
use super::metadata_receiver::{MetadataReceiver, PresetFile};
use super::{Location, LocationValue, PresetDiscoveryFactory, ProviderMetadata};
use crate::cli::tracing::{Span, record};
use crate::plugin::util::{Proxy, clap_call};
use anyhow::{Context, Result};
use clap_sys::factory::preset_discovery::clap_preset_discovery_provider;
use std::collections::{BTreeMap, HashSet};
use std::ffi::CString;
use std::marker::PhantomData;
use std::ptr::NonNull;
use walkdir::WalkDir;

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
    _indexer: Proxy<Indexer>,
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

        let provider_id_cstring = CString::new(provider_id).expect("The provider ID contained internal null bytes");
        let provider = {
            let span = Span::begin(
                "clap_preset_discovery_factory::create",
                record! {
                    provider_id: provider_id
                },
            );

            let factory = factory.as_ptr();
            let provider = unsafe {
                clap_call! {
                    factory=>create(
                        factory,
                        Proxy::vtable(&indexer),
                        provider_id_cstring.as_ptr()
                    )
                }
            };

            span.finish(record!(result: format_args!("{:p}", provider)));

            match NonNull::new(provider as *mut clap_preset_discovery_provider) {
                Some(provider) => provider,
                None => anyhow::bail!(
                    "'clap_preset_discovery_factory::create()' returned a null pointer for the provider with ID \
                     '{provider_id}'.",
                ),
            }
        };

        let declared_data = {
            let provider = provider.as_ptr();

            let span = Span::begin("clap_preset_discovery_provider::init", ());
            let result = unsafe {
                clap_call! { provider=>init(provider) }
            };

            span.finish(record!(result: result));

            if !result {
                anyhow::bail!(
                    "'clap_preset_discovery_provider::init()' returned false for the provider with ID '{provider_id}'."
                );
            }

            indexer.finish().with_context(|| {
                format!(
                    "Errors produced during 'clap_preset_discovery_indexer' callbacks made by the provider with ID \
                     '{provider_id}'"
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
            anyhow::bail!("The 'desc' field on the 'clap_preset_provider' struct is a null pointer.");
        }

        unsafe { ProviderMetadata::from_descriptor(descriptor) }
    }

    /// Get the raw pointer to the `clap_preset_discovery_provider` instance.
    pub fn as_ptr(&self) -> *const clap_preset_discovery_provider {
        self.handle.as_ptr()
    }

    /// Get the data declared by the provider during its initialization.
    pub fn declared_data(&self) -> &IndexerResults {
        &self.declared_data
    }

    /// Crawl a location for presets. If the location is a directory, then this walks that directory
    /// and queries metadata for each preset that matches the declared file extensions. The location
    /// must be obtained from [`declared_data()`][Self::declared_data()]. Returns an error if the
    /// plugin triggered any kind of error. The returned map contains a [`PresetFile`] for each of
    /// the crawled locations that the plugin declared presets for, which can be either a single
    /// preset or a container of multiple presets.
    pub fn crawl_location(&self, location: &Location) -> Result<BTreeMap<LocationValue, PresetFile>> {
        let mut results = BTreeMap::new();

        let location_flags = location.flags;
        let mut crawl = |location: LocationValue| -> Result<()> {
            let (location_kind, location_ptr) = location.to_raw();

            let metadata_receiver = MetadataReceiver::new(location.clone(), location_flags);
            let provider = self.as_ptr();

            let span = Span::begin(
                "clap_preset_discovery_provider::get_metadata",
                record! {
                    location: location,
                    location_flags: location_flags
                },
            );

            let result = unsafe {
                clap_call! {
                    provider=>get_metadata(
                        provider,
                        location_kind,
                        location_ptr,
                        Proxy::vtable(&metadata_receiver)
                    )
                }
            };

            span.finish(record!(result: result));

            if !result {
                anyhow::bail!("The preset provider returned false when fetching metadata for {location}.",);
            }

            let result = metadata_receiver
                .finish()
                .with_context(|| format!("Error while fetching fetching metadata for {location}"))?;

            if let Some(preset_file) = result {
                results.insert(location, preset_file);
            }

            Ok(())
        };

        match location.value.file_path() {
            Some(file_path) => {
                // Single files are queried as is, directories are crawled. If the declared location
                // does not exist, then that results in a hard error.
                let metadata = std::fs::metadata(&file_path)
                    .with_context(|| "Could not query metadata for the declared file location '{file_path}'")?;
                if metadata.is_dir() {
                    // If the plugin declared valid file extensions, then we'll filter by those file
                    // extensions
                    let allowed_extensions: HashSet<_> = self
                        .declared_data
                        .file_types
                        .iter()
                        .map(|file_type| file_type.extension.as_str())
                        .collect();

                    let walker = WalkDir::new(file_path)
                        .min_depth(1)
                        .follow_links(true)
                        .same_file_system(false)
                        .into_iter()
                        .filter_map(|entry| entry.ok())
                        .filter(|entry| entry.file_type().is_file())
                        .filter(|entry| {
                            allowed_extensions.is_empty()
                                || matches!(entry.path().extension(), Some(extension)
                                               if allowed_extensions.contains(extension.to_str().unwrap()))
                        });

                    for candidate in walker {
                        assert!(candidate.path().is_absolute());

                        // TODO: Not quite sure what should be done with errors when crawling
                        //       directories. If the plugin doesn't return an error but also doesn't
                        //       declare any presets then that gets handled gracefully
                        crawl(LocationValue::File(
                            CString::new(candidate.path().to_string_lossy().to_string())
                                .context("File path contains nul byte")?,
                        ))?;
                    }
                } else {
                    crawl(location.value.clone())?;
                }
            }
            None => {
                crawl(LocationValue::Internal)?;
            }
        }

        Ok(results)
    }
}
