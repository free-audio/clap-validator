//! A wrapper around `clap_preset_discovery_provider`.

use anyhow::{Context, Result};
use std::collections::{BTreeMap, HashSet};
use std::ffi::CString;
use std::marker::PhantomData;
use std::pin::Pin;
use std::ptr::NonNull;
use walkdir::WalkDir;

use clap_sys::factory::draft::preset_discovery::clap_preset_discovery_provider;

use super::indexer::{Indexer, IndexerResults};
use super::metadata_receiver::{MetadataReceiver, PresetFile};
use super::{Location, LocationUri, PresetDiscoveryFactory, ProviderMetadata};
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
                "The 'desc' field on the 'clap_preset_provider' struct is a null pointer."
            );
        }

        ProviderMetadata::from_descriptor(unsafe { &*descriptor })
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
    /// the crawled URIs that the plugin declared presets for, which can be either a single preset
    /// or a container of multiple presets.
    pub fn crawl_location(&self, location: &Location) -> Result<BTreeMap<String, PresetFile>> {
        let mut results = BTreeMap::new();

        let mut crawl_uri = |uri: String| -> Result<()> {
            let uri_cstring = CString::new(uri.clone()).context("Invalid UTF-8 in URI")?;

            // There is no 'end of preset' kind of function in the metadata provider, so when
            // the `MetadataReceiver` is dropped it may still need to write a preset file or
            // emit some errors. That's why it borrows this result, and writes the output
            // theere. This can happen during the drop.
            let mut result = None;
            {
                let metadata_receiver = MetadataReceiver::new(&mut result, location);

                let provider = self.as_ptr();
                let success = unsafe_clap_call! {
                    provider=>get_metadata(
                        provider,
                        uri_cstring.as_ptr(),
                        metadata_receiver.clap_preset_discovery_metadata_receiver_ptr()
                    )
                };
                if !success {
                    // TODO: Is the plugin allowed to return false here? If it doesn't have any
                    //       presets it should just not declare any, right?
                    anyhow::bail!(
                        "The preset provider returned false when fetching metadata for the URI \
                         '{uri}'.",
                    );
                }
            }

            if let Some(preset_file) = result {
                let preset_file = preset_file.with_context(|| {
                    format!("Error while fetching fetching metadata for the URI '{uri}'",)
                })?;

                results.insert(uri, preset_file);
            }

            Ok(())
        };

        match &location.uri {
            LocationUri::File(file_path) => {
                // Single files are queried as is, directories are crawled. If the declared location
                // does not exist, then that results in a hard error.
                let metadata = std::fs::metadata(file_path).with_context(|| {
                    "Could not query metadata for the declared file location '{file_path}'"
                })?;
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
                        let uri = format!("file://{}", candidate.path().to_str().unwrap());

                        // I'm not actually sure if `PathBuf`s always use forward slashes or not,
                        // but while the original URI does use forward slashes this candidate path
                        // may not.
                        #[cfg(windows)]
                        let uri = uri.replace('\\', "/");

                        // TODO: Not quite sure what should be done with errors when crawling
                        //       directories. If the plugin doesn't return an error but also doesn't
                        //       declare any presets then that gets handled gracefully
                        crawl_uri(uri)?;
                    }
                } else {
                    crawl_uri(location.uri.to_uri())?;
                }
            }
            LocationUri::Internal => {
                crawl_uri(location.uri.to_uri())?;
            }
        }

        Ok(results)
    }
}
