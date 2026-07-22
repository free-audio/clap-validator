//! Basic smoke tests for optional CLAP extensions (Tier A).
//!
//! Each test skips when the plugin does not implement the extension.

use anyhow::{Context, Result};

use crate::plugin::ext::latency::Latency;
use crate::plugin::ext::render::Render;
use crate::plugin::ext::tail::Tail;
use crate::plugin::ext::Extension;
use crate::plugin::host::Host;
use crate::plugin::library::PluginLibrary;
use crate::tests::TestStatus;

const SAMPLE_RATE: f64 = 44_100.0;
const MIN_BUFFER: usize = 1;
const MAX_BUFFER: usize = 512;

/// The test for `PluginTestCase::LatencyBasic`.
pub fn test_latency_basic(library: &PluginLibrary, plugin_id: &str) -> Result<TestStatus> {
    let host = Host::new();
    let plugin = library
        .create_plugin(plugin_id, host.clone())
        .context("Could not create the plugin instance")?;

    plugin.init().context("Error during initialization")?;
    let latency = match plugin.get_extension::<Latency>() {
        Some(latency) => latency,
        None => {
            return Ok(TestStatus::Skipped {
                details: Some(format!(
                    "The plugin does not implement the '{}' extension.",
                    Latency::EXTENSION_ID.to_str().unwrap(),
                )),
            });
        }
    };
    host.handle_callbacks_once();

    // Spec (CLAP 1.2.x): latency may only be queried while activated.
    plugin
        .activate(SAMPLE_RATE, MIN_BUFFER, MAX_BUFFER)
        .context("Error while activating the plugin")?;
    host.handle_callbacks_once();

    let samples = latency
        .get()
        .context("Error while querying 'clap_plugin_latency::get()'")?;
    host.handle_callbacks_once();
    host.callback_error_check()
        .context("An error occured during a host callback")?;

    plugin.deactivate();

    Ok(TestStatus::Success {
        details: Some(format!("Reported latency: {samples} samples")),
    })
}

/// The test for `PluginTestCase::TailBasic`.
pub fn test_tail_basic(library: &PluginLibrary, plugin_id: &str) -> Result<TestStatus> {
    let host = Host::new();
    let plugin = library
        .create_plugin(plugin_id, host.clone())
        .context("Could not create the plugin instance")?;

    plugin.init().context("Error during initialization")?;
    let tail = match plugin.get_extension::<Tail>() {
        Some(tail) => tail,
        None => {
            return Ok(TestStatus::Skipped {
                details: Some(format!(
                    "The plugin does not implement the '{}' extension.",
                    Tail::EXTENSION_ID.to_str().unwrap(),
                )),
            });
        }
    };
    host.handle_callbacks_once();

    plugin
        .activate(SAMPLE_RATE, MIN_BUFFER, MAX_BUFFER)
        .context("Error while activating the plugin")?;
    host.handle_callbacks_once();

    let samples = tail
        .get()
        .context("Error while querying 'clap_plugin_tail::get()'")?;
    host.handle_callbacks_once();
    host.callback_error_check()
        .context("An error occured during a host callback")?;

    plugin.deactivate();

    Ok(TestStatus::Success {
        details: Some(format!("Reported tail: {samples} samples")),
    })
}

/// The test for `PluginTestCase::RenderModes`.
pub fn test_render_modes(library: &PluginLibrary, plugin_id: &str) -> Result<TestStatus> {
    let host = Host::new();
    let plugin = library
        .create_plugin(plugin_id, host.clone())
        .context("Could not create the plugin instance")?;

    plugin.init().context("Error during initialization")?;
    let render = match plugin.get_extension::<Render>() {
        Some(render) => render,
        None => {
            return Ok(TestStatus::Skipped {
                details: Some(format!(
                    "The plugin does not implement the '{}' extension.",
                    Render::EXTENSION_ID.to_str().unwrap(),
                )),
            });
        }
    };
    host.handle_callbacks_once();

    let hard_rt = render
        .has_hard_realtime_requirement()
        .context("Error while querying 'has_hard_realtime_requirement()'")?;

    // Realtime should be supported by any non-broken implementation.
    let realtime_ok = render
        .set_realtime()
        .context("Error while setting CLAP_RENDER_REALTIME")?;
    if !realtime_ok {
        anyhow::bail!(
            "'clap_plugin_render::set(CLAP_RENDER_REALTIME)' returned false; realtime mode \
             should be supported."
        );
    }

    // Offline is optional — false means "not supported", not a failure.
    let offline_ok = render
        .set_offline()
        .context("Error while setting CLAP_RENDER_OFFLINE")?;

    // Leave the plugin in realtime mode for any subsequent host lifecycle.
    let _ = render.set_realtime()?;

    host.handle_callbacks_once();
    host.callback_error_check()
        .context("An error occured during a host callback")?;

    Ok(TestStatus::Success {
        details: Some(format!(
            "hard_realtime_requirement={hard_rt}, offline_supported={offline_ok}"
        )),
    })
}
