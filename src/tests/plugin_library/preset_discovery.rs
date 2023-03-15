//! Tests involving the preset discovery factory.

use anyhow::{Context, Result};
use clap_sys::factory::draft::preset_discovery::CLAP_PRESET_DISCOVERY_FACTORY_ID;
use std::collections::BTreeMap;
use std::path::Path;

use crate::plugin::ext::audio_ports::AudioPorts;
use crate::plugin::ext::preset_load::PresetLoad;
use crate::plugin::ext::Extension;
use crate::plugin::host::Host;
use crate::plugin::instance::process::ProcessConfig;
use crate::plugin::library::PluginLibrary;
use crate::plugin::preset_discovery::{LocationValue, PluginAbi, Preset, PresetFile};
use crate::tests::plugin::ProcessingTest;
use crate::tests::TestStatus;

// TODO: Test for duplicate locations and soundpacks in declared data across all providers

/// The fixed buffer size to use for these tests.
const BUFFER_SIZE: usize = 512;

/// The test for `PluginLibraryTestCase::PresetDiscoveryCrawl`. Makes sure that all of a plugin's
/// reported preset locations can be crawled successfully. If `load_presets` is enabled, then the
/// crawled presets are also loaded.
pub fn test_crawl(library_path: &Path, load_presets: bool) -> Result<TestStatus> {
    let library = PluginLibrary::load(library_path)
        .with_context(|| format!("Could not load '{}'", library_path.display()))?;
    let preset_discovery_factory = match library.preset_discovery_factory() {
        Ok(preset_discovery_factory) => preset_discovery_factory,
        Err(_) => {
            return Ok(TestStatus::Skipped {
                details: Some(format!(
                    "The plugin does not implement the '{}' factory.",
                    CLAP_PRESET_DISCOVERY_FACTORY_ID.to_str().unwrap(),
                )),
            })
        }
    };

    // All found presets, indexed by location (value)
    let mut found_presets: BTreeMap<LocationValue, PresetFile> = BTreeMap::new();

    let metadata = preset_discovery_factory
        .metadata()
        .context("Could not fetch the preset provider descriptors from the factory")?;
    for provider_metadata in metadata {
        let provider = preset_discovery_factory
            .create_provider(&provider_metadata)
            .with_context(|| {
                format!(
                    "Could not create the provider with ID '{}'",
                    provider_metadata.id
                )
            })?;
        for location in &provider.declared_data().locations {
            let presets = provider.crawl_location(location).with_context(|| {
                format!(
                    "Error occurred while crawling presets for the location '{}' with {} using \
                     provider '{}' with ID '{}'",
                    location.name, location.value, provider_metadata.name, provider_metadata.id,
                )
            })?;
            found_presets.extend(presets);
        }
    }

    // After crawling, group the presets by CLAP plugin ID and try to load them
    if load_presets {
        // Because container presets can contain presets for multiple different plugins storing all
        // presets grouped by plugin ID is not possible by storing `PresetFiles`s. So this is a
        // simple wrapper around `PresetFile` to use with the preset load extension. The `Preset` is
        // technically not needed anymore but it's nice for error reporting.
        struct LoadablePreset {
            location: LocationValue,
            load_key: Option<String>,
            preset: Preset,
        }

        // Stores `PresetFile`s with their associated locations for all CLAP plugin IDs in
        // `found_presets`
        let mut loadable_presets_by_plugin_id: BTreeMap<String, Vec<LoadablePreset>> =
            BTreeMap::new();
        let mut maybe_add_preset =
            |location: &LocationValue, load_key: Option<String>, preset: Preset| {
                for plugin_id in &preset.plugin_ids {
                    if plugin_id.abi == PluginAbi::Clap {
                        if !loadable_presets_by_plugin_id.contains_key(&plugin_id.id) {
                            loadable_presets_by_plugin_id.insert(plugin_id.id.clone(), Vec::new());
                        }

                        loadable_presets_by_plugin_id
                            .get_mut(&plugin_id.id)
                            .unwrap()
                            .push(LoadablePreset {
                                location: location.clone(),
                                load_key: load_key.clone(),
                                preset: preset.clone(),
                            })
                    }
                }
            };

        for (location, preset_file) in found_presets {
            match preset_file {
                PresetFile::Single(preset) => maybe_add_preset(&location, None, preset),
                PresetFile::Container(presets) => {
                    for (load_key, preset) in presets {
                        maybe_add_preset(&location, Some(load_key), preset);
                    }
                }
            }
        }

        // With everything indexed, we can try loading these presets. We'll reuse one plugin
        // instance per plugin.
        for (plugin_id, presets) in loadable_presets_by_plugin_id {
            let host = Host::new();
            let plugin = library
                .create_plugin(&plugin_id, host.clone())
                .with_context(|| format!("Could not create a plugin instance for '{plugin_id}'"))?;
            plugin
                .init()
                .with_context(|| format!("Error while initializing '{plugin_id}'"))?;

            let preset_load = match plugin.get_extension::<PresetLoad>() {
                Some(preset_load) => preset_load,
                None => {
                    return Ok(TestStatus::Skipped {
                        details: Some(format!(
                            "'{}' does not implement the '{}' extension.",
                            plugin_id,
                            PresetLoad::EXTENSION_ID.to_str().unwrap(),
                        )),
                    })
                }
            };
            // We'll try to run some audio through the plugin to make sure the preset change was
            // successful, but it doesn't matter if the plugin doesn't have any audio ports
            let audio_ports = plugin.get_extension::<AudioPorts>();
            host.handle_callbacks_once();

            let audio_ports_config = audio_ports
                .map(|ports| ports.config())
                .transpose()
                .context("Could not fetch the plugin's audio port config")?;
            let (mut input_buffers, mut output_buffers) = audio_ports_config
                .unwrap_or_default()
                .create_buffers(BUFFER_SIZE);

            for LoadablePreset {
                location,
                load_key,
                preset,
            } in presets
            {
                // TODO: We now always deactivate the plugin before loading presets, but presets can
                //       be loaded at any point, even when the plugin is processing audio. Test
                //       this.
                let load_result = preset_load
                    .from_location(&location, load_key.as_deref())
                    .with_context(|| {
                        format!(
                            "Could not load the preset '{}' for plugin '{}'",
                            preset.name, plugin_id
                        )
                    });

                // In case the plugin uses `clap_host_preset_load::on_error()` to report an error,
                // we will check that first before making sure the preset loaded correctly. This
                // might otherwise mask the error message.
                host.handle_callbacks_once();
                host.callback_error_check().with_context(|| {
                    format!(
                        "An error occurred while loading the preset '{}' for plugin '{}'",
                        preset.name, plugin_id
                    )
                })?;
                // See above
                load_result?;

                // We'll process a single buffer of silent audio just to make sure everything's
                // settled in
                ProcessingTest::new_out_of_place(&plugin, &mut input_buffers, &mut output_buffers)?
                    .run_once(ProcessConfig::default(), |_| Ok(()))
                    .with_context(|| {
                        format!(
                            "Error while processing an audio buffer after loading a preset for \
                             '{plugin_id}'"
                        )
                    })?;

                host.handle_callbacks_once();
                host.callback_error_check().with_context(|| {
                    format!("An error occured during a host callback made by '{plugin_id}'")
                })?;
            }

            host.handle_callbacks_once();
            host.callback_error_check().with_context(|| {
                format!("An error occured during a host callback made by '{plugin_id}'")
            })?;
        }
    }

    Ok(TestStatus::Success { details: None })
}

/// The test for `PluginLibraryTestCase::PresetDiscoveryDescriptorConsistency`. Verifies that the
/// descriptors stored in a plugin's preset providers match those returned by the factory.
pub fn test_descriptor_consistency(library_path: &Path) -> Result<TestStatus> {
    let library = PluginLibrary::load(library_path)
        .with_context(|| format!("Could not load '{}'", library_path.display()))?;
    let preset_discovery_factory = match library.preset_discovery_factory() {
        Ok(preset_discovery_factory) => preset_discovery_factory,
        Err(_) => {
            return Ok(TestStatus::Skipped {
                details: Some(format!(
                    "The plugin does not implement the '{}' factory.",
                    CLAP_PRESET_DISCOVERY_FACTORY_ID.to_str().unwrap(),
                )),
            })
        }
    };

    let metadata = preset_discovery_factory
        .metadata()
        .context("Could not fetch the preset provider descriptors from the factory")?;
    for factory_metadata in metadata {
        let provider = preset_discovery_factory
            .create_provider(&factory_metadata)
            .with_context(|| {
                format!(
                    "Could not create the provider with ID '{}'",
                    factory_metadata.id
                )
            })?;
        let provider_metadata = provider.descriptor().with_context(|| {
            format!(
                "Could not grab the descriptor from the 'clap_preset_discovery_provider''s 'desc' \
                 field for '{}'",
                &factory_metadata.id
            )
        })?;

        if provider_metadata != factory_metadata {
            anyhow::bail!(
                "The 'clap_preset_discovery_provider_descriptor' stored on '{}'s \
                 'clap_preset_discovery_provider' object contains different values than the one \
                 returned by the factory.",
                factory_metadata.id
            );
        }
    }

    Ok(TestStatus::Success { details: None })
}
