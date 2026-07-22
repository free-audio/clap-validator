//! Tests for entire plugin libraries. These are mostly used to test plugin scanning behavior.

use crate::cli::tracing::{Span, record};
use crate::tests::TestStatus;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

mod factories;
mod preset_discovery;
mod scanning;

/// Tests for entire CLAP libraries. These are mostly to ensure good plugin scanning practices. See
/// the module's heading for more information, and the `description` function below for a
/// description of each test case.
#[derive(
    strum_macros::Display,
    strum_macros::EnumString,
    strum_macros::EnumIter,
    strum_macros::IntoStaticStr,
    Serialize,
    Deserialize,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum PluginLibraryTestCase {
    QueryNonexistentFactory,
    CreateIdWithTrailingGarbage,
    ScanRtldNow,
    ScanTime,
    PresetDiscoveryCrawl,
    PresetDiscoveryDescriptorConsistency,
    PresetDiscoveryLoad,
}

impl PluginLibraryTestCase {
    pub fn description(&self) -> String {
        match self {
            Self::PresetDiscoveryCrawl => String::from(
                "If the plugin supports the preset discovery mechanism, then this test ensures that all of the \
                 plugin's declared locations can be indexed successfully.",
            ),
            Self::PresetDiscoveryDescriptorConsistency => String::from(
                "Ensures that all preset provider descriptors from a preset discovery factory match those stored in \
                 the providers created by the factory.",
            ),
            Self::PresetDiscoveryLoad => format!(
                "The same as '{}', but also tries to load all found presets for plugins supported the CLAP plugin \
                 library. A single plugin instance is reused for loading multiple presets, and the process function \
                 is called after loading each preset.",
                Self::PresetDiscoveryCrawl
            ),
            Self::ScanTime => format!(
                "Checks whether the plugin can be scanned in under {} milliseconds.",
                scanning::SCAN_TIME_LIMIT.as_millis()
            ),
            Self::ScanRtldNow => String::from(
                "Checks whether the plugin loads correctly when loaded using 'dlopen(..., RTLD_LOCAL | RTLD_NOW)'. \
                 Only run on Unix-like platforms.",
            ),
            Self::QueryNonexistentFactory => String::from(
                "Tries to query a factory from the plugin's entry point with a non-existent ID. This should return a \
                 null pointer.",
            ),
            Self::CreateIdWithTrailingGarbage => String::from(
                "Attempts to create a plugin instance using an existing plugin ID with some extra text appended to \
                 the end. This should return a null pointer.",
            ),
        }
    }

    pub fn run(&self, library_path: &Path) -> Result<TestStatus> {
        let _span = Span::begin(
            self.into(),
            record! {
                library_path: library_path.display().to_string()
            },
        );

        match self {
            Self::PresetDiscoveryCrawl => preset_discovery::test_crawl(library_path, false),
            Self::PresetDiscoveryDescriptorConsistency => preset_discovery::test_descriptor_consistency(library_path),
            Self::PresetDiscoveryLoad => preset_discovery::test_crawl(library_path, true),
            Self::ScanTime => scanning::test_scan_time(library_path),
            Self::ScanRtldNow => scanning::test_scan_rtld_now(library_path),
            Self::QueryNonexistentFactory => factories::test_query_nonexistent_factory(library_path),
            Self::CreateIdWithTrailingGarbage => factories::test_create_id_with_trailing_garbage(library_path),
        }
    }
}
