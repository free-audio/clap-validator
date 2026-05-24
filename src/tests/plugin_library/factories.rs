//! Tests interacting with the plugin's factories.

use anyhow::{Context, Result};
use clap_sys::version::clap_version_is_compatible;
use std::path::Path;

use crate::plugin::host::Host;
use crate::plugin::library::PluginLibrary;
use crate::tests::TestStatus;

/// The test for `PluginLibraryTestCase::QueryNonexistentFactory`.
pub fn test_query_nonexistent_factory(library_path: &Path) -> Result<TestStatus> {
    let library = PluginLibrary::load(library_path)
        .with_context(|| format!("Could not load '{}'", library_path.display()))?;

    // This should be actually random instead of using a fixed seed like the other tests. This
    // factory ID may not be used by anything.
    let nonexistent_factory_id = format!("foo-factory-{}", rand::random::<u64>());
    let nonexistent_factory_exists = library.factory_exists(&nonexistent_factory_id);

    // Since this factory doesn't exist, the plugin should always return a null pointer.
    if nonexistent_factory_exists {
        anyhow::bail!(
            "Querying a factory with the non-existent factory ID '{nonexistent_factory_id} should \
             return a null pointer, but the plugin returned a non-null pointer instead. The \
             plugin may be unconditionally returning the plugin factory."
        );
    } else {
        Ok(TestStatus::Success { details: None })
    }
}

/// The test for `PluginLibraryTestCase::CreateIdWithTrailingGarbage`.
pub fn test_create_id_with_trailing_garbage(library_path: &Path) -> Result<TestStatus> {
    let library = PluginLibrary::load(library_path)
        .with_context(|| format!("Could not load '{}'", library_path.display()))?;

    let metadata = library
        .metadata()
        .context("Could not query the plugin's metadata")?;
    if !clap_version_is_compatible(metadata.clap_version()) {
        return Ok(TestStatus::Skipped {
            details: Some(format!(
                "'{}' uses an unsupported CLAP version ({}.{}.{})",
                library_path.display(),
                metadata.version.0,
                metadata.version.1,
                metadata.version.2
            )),
        });
    }

    // We'll ask the plugin to create an instance of a plugin with the same ID as the first one from
    // the factory, but with some additional data appended to the end. Since the plugin doesn't
    // exist, this should return a null pointer.
    let fake_plugin_id = match metadata.plugins.first() {
        Some(descriptor) => {
            // The x makes it cooler. And we'll try 100 versions in case the cooler
            // verion of the plugin already exists.
            let fake_plugin_id = (1..=100)
                .map(|n| format!("{}x{n}", descriptor.id))
                .find(|candidate| !metadata.plugins.iter().any(|d| &d.id == candidate));

            match fake_plugin_id {
                Some(fake_plugin_id) => fake_plugin_id,
                // This obviously should never be triggered unless someone is
                // intentionally triggering it
                None => {
                    return Ok(TestStatus::Skipped {
                        details: Some(String::from(
                            "All of the coolest plugins already exists. In other words, could not \
                             come up a fake unused plugin ID.",
                        )),
                    });
                }
            }
        }
        None => {
            return Ok(TestStatus::Skipped {
                details: Some(String::from(
                    "The plugin library does not expose any plugins",
                )),
            });
        }
    };

    // This should return an error/null-pointer instead of actually instantiating a
    // plugin
    if library.create_plugin(&fake_plugin_id, Host::new()).is_ok() {
        anyhow::bail!(
            "Creating a plugin instance with a non-existent plugin ID '{fake_plugin_id}' should \
             return a null pointer, but it did not."
        );
    } else {
        Ok(TestStatus::Success { details: None })
    }
}
