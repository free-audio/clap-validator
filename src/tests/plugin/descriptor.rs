//! Tests surrounding plugin features.

use anyhow::{Context, Result};
use clap_sys::plugin_features::{
    CLAP_PLUGIN_FEATURE_ANALYZER, CLAP_PLUGIN_FEATURE_AUDIO_EFFECT, CLAP_PLUGIN_FEATURE_INSTRUMENT,
    CLAP_PLUGIN_FEATURE_NOTE_DETECTOR, CLAP_PLUGIN_FEATURE_NOTE_EFFECT,
};
use std::collections::HashSet;

use crate::plugin::host::Host;
use crate::plugin::library::PluginLibrary;
use crate::tests::TestStatus;

/// Verifies that the descriptor stored in the factory and the descriptor stored on the plugin
/// object are equivalent.
pub fn test_consistency(library: &PluginLibrary, plugin_id: &str) -> Result<TestStatus> {
    let metadata = library.metadata().with_context(|| {
        format!(
            "Could not fetch plugin metadata for '{}'",
            library.plugin_path().display()
        )
    })?;
    let factory_descriptor = metadata
        .plugins
        .into_iter()
        .find(|plugin_meta| plugin_meta.id == plugin_id)
        .expect("Incorrect plugin ID for metadata query, this is a bug in clap-validator");

    let host = Host::new();
    let plugin = library
        .create_plugin(plugin_id, host)
        .context("Could not create the plugin instance")?;
    let plugin_descriptor = plugin.descriptor()?;

    if plugin_descriptor == factory_descriptor {
        Ok(TestStatus::Success { details: None })
    } else {
        Ok(TestStatus::Failed {
            details: Some(format!(
                "The 'clap_plugin_descriptor' stored on '{plugin_id}'s 'clap_plugin' object \
                 contains different values than the one returned by the factory."
            )),
        })
    }
}

/// Check whether the plugin's categories are consistent. Currently this just makes sure that the
/// plugin has one of the four main plugin category features.
pub fn test_features_categories(library: &PluginLibrary, plugin_id: &str) -> Result<TestStatus> {
    let features = plugin_features(library, plugin_id)?;

    // These are stored in the bindings as C-compatible null terminated strings, but we'll need them
    // as regular string slices so we can compare them to
    let instrument_feature = CLAP_PLUGIN_FEATURE_INSTRUMENT.to_str().unwrap();
    let audio_effect_feature = CLAP_PLUGIN_FEATURE_AUDIO_EFFECT.to_str().unwrap();
    let note_detector_feature = CLAP_PLUGIN_FEATURE_NOTE_DETECTOR.to_str().unwrap();
    let note_effect_feature = CLAP_PLUGIN_FEATURE_NOTE_EFFECT.to_str().unwrap();
    let analyzer_feature = CLAP_PLUGIN_FEATURE_ANALYZER.to_str().unwrap();

    let has_main_category = features.iter().any(|feature| -> bool {
        feature == instrument_feature
            || feature == audio_effect_feature
            || feature == note_detector_feature
            || feature == note_effect_feature
            || feature == analyzer_feature
    });

    if has_main_category {
        Ok(TestStatus::Success { details: None })
    } else {
        anyhow::bail!(
            "The plugin needs to have at least one of thw following plugin category features: \
             \"{instrument_feature}\", \"{audio_effect_feature}\", \"{note_effect_feature}\", or \
             \"{analyzer_feature}\"."
        )
    }
}

/// Confirm that the plugin does not have any duplicate features.
pub fn test_features_duplicates(library: &PluginLibrary, plugin_id: &str) -> Result<TestStatus> {
    let mut features = plugin_features(library, plugin_id)?;
    let unique_features: HashSet<&str> = features.iter().map(|feature| feature.as_str()).collect();

    if unique_features.len() == features.len() {
        Ok(TestStatus::Success { details: None })
    } else {
        // We'll sort the features first to make spotting the duplicates easier
        features.sort_unstable();

        anyhow::bail!("The plugin has duplicate features: {features:?}")
    }
}

/// Get the feature vector for a plugin in the library. Returns `None` if the plugin ID does not
/// exist in the library.
fn plugin_features(library: &PluginLibrary, plugin_id: &str) -> Result<Vec<String>> {
    library
        .metadata()
        .with_context(|| {
            format!(
                "Could not fetch plugin metadata for '{}'",
                library.plugin_path().display()
            )
        })
        .and_then(|metadata| {
            metadata
                .plugins
                .into_iter()
                .find(|plugin_meta| plugin_meta.id == plugin_id)
                .context("Incorrect plugin ID for metadata query, this is a bug in clap-validator")
        })
        .map(|metadata| metadata.features)
}
