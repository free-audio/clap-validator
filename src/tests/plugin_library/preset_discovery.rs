//! Tests involving the preset discovery factory.

use anyhow::{Context, Result};
use clap_sys::factory::draft::preset_discovery::CLAP_PRESET_DISCOVERY_FACTORY_ID;
use std::path::Path;

use crate::plugin::library::PluginLibrary;
use crate::tests::TestStatus;

// TODO: Test that all declared presets are in fact loadable

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
                    "The plugin does not implement the '{}' factory",
                    CLAP_PRESET_DISCOVERY_FACTORY_ID.to_str().unwrap(),
                )),
            })
        }
    };

    let metadata = preset_discovery_factory
        .metadata()
        .context("Could not fetch the preset provider descriptors from the factory")?;
    for factory_metadata in metadata {
        let provider = preset_discovery_factory.create_provider(&factory_metadata)?;
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
