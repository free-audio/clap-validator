//! Tests involving plugin scanning.

use anyhow::{Context, Result};
use clap_sys::version::clap_version_is_compatible;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::plugin::library::PluginLibrary;
use crate::tests::TestStatus;

pub const SCAN_TIME_LIMIT: Duration = Duration::from_millis(100);

/// The test for `PluginLibraryTestCase::ScanTime`.
pub fn test_scan_time(library_path: &Path) -> Result<TestStatus> {
    let test_start = Instant::now();

    {
        // The library will be unloaded when this object is dropped, so that is part of the
        // measurement
        let library =
            PluginLibrary::load(library_path).with_context(|| format!("Could not load '{}'", library_path.display()));

        // This goes through all plugins and builds a data structure containing information for all
        // of those plugins, mimicing most of a DAW's plugin scanning process
        let metadata = library.and_then(|plugin_library| {
            plugin_library
                .metadata()
                .context("Could not query the plugin's metadata")
        })?;
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
    }

    let test_end = Instant::now();
    let init_duration = test_end - test_start;
    if init_duration <= SCAN_TIME_LIMIT {
        let millis = init_duration.as_millis();
        Ok(TestStatus::Success {
            details: Some(format!(
                "The plugin can be scanned in {} {}.",
                millis,
                if millis == 1 { "millisecond" } else { "milliseconds" }
            )),
        })
    } else {
        // This should not be treated as a fatal error since the scanning time will dependon the
        // system
        Ok(TestStatus::Warning {
            details: Some(format!(
                "The plugin took {} milliseconds to scan",
                init_duration.as_millis()
            )),
        })
    }
}

/// The test for `PluginLibraryTestCase::ScanRtldNow`.
#[cfg(unix)]
pub fn test_scan_rtld_now(library_path: &Path) -> Result<TestStatus> {
    // The plugin may have issues resolving certain symbols. This should help catch this upfront.
    PluginLibrary::load_with(library_path, |path| {
        unsafe {
            libloading::os::unix::Library::open(
                Some(path),
                libloading::os::unix::RTLD_LOCAL | libloading::os::unix::RTLD_NOW,
            )
        }
        .map(libloading::Library::from)
        .context("Could not load the plugin library using 'RTLD_LOCAL | RTLD_NOW'")
    })
    .with_context(|| format!("Could not load '{}' using 'RTLD_NOW", library_path.display()))?;

    Ok(TestStatus::Success { details: None })
}

#[cfg(not(unix))]
pub fn test_scan_rtld_now(_: &Path) -> Result<TestStatus> {
    Ok(TestStatus::Skipped {
        details: Some(String::from("This test is only relevant to Unix-like platforms")),
    })
}
