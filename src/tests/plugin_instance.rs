//! Tests for individual plugin instances.

use crate::cli::tracing::{Span, record};
use crate::plugin::library::PluginLibrary;
use crate::tests::TestStatus;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

mod descriptor;
mod layout;
mod params;
mod processing;
mod state;
mod transport;

/// The tests for individual CLAP plugins. See the module's heading for more information, and the
/// `description` function below for a description of each test case.
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
pub enum PluginInstanceTestCase {
    DescriptorConsistency,
    FeaturesCategories,
    FeaturesDuplicates,
    ProcessAudioBasicOutOfPlace,
    ProcessAudioBasicInPlace,
    ProcessAudioDoubleOutOfPlace,
    ProcessAudioDoubleInPlace,
    ProcessAudioDenormals,
    ProcessSleepConstantMask,
    ProcessSleepProcessStatus,
    ProcessNoteOutOfPlaceBasic,
    ProcessNoteInconsistent,
    ProcessNoteWildcard,
    ProcessVaryingSampleRates,
    ProcessVaryingBlockSizes,
    ProcessRandomBlockSizes,
    ProcessResetReactivate,
    LayoutAudioPortsActivation,
    LayoutAudioPortsConfig,
    LayoutConfigurableAudioPorts,
    ParamSetEvents,
    ParamSetNoCookies,
    ParamSetWrongNamespace,
    ParamFuzzBasic,
    ParamFuzzBounds,
    ParamFuzzSampleAccurate,
    ParamFuzzModulation,
    ParamConversions,
    ParamDefaultValues,
    StateInvalidEmpty,
    StateInvalidRandom,
    StateReproducibilityBasic,
    StateReproducibilityBinary,
    StateReproducibilityBuffered,
    TransportNull,
    TransportFuzz,
    TransportFuzzSampleAccurate,
}

impl PluginInstanceTestCase {
    pub fn description(&self) -> String {
        match self {
            Self::DescriptorConsistency => String::from(
                "The plugin descriptor returned from the plugin factory and the plugin descriptor stored on the \
                 'clap_plugin' object should be equivalent.",
            ),
            Self::FeaturesCategories => {
                String::from("The plugin needs to have at least one of the main CLAP category features.")
            }
            Self::FeaturesDuplicates => String::from("The plugin's features array should not contain any duplicates."),
            Self::ProcessAudioBasicOutOfPlace => String::from(
                "Processes random audio through the plugin with its default parameter values and tests whether the \
                 output does not contain any non-finite or subnormal values. Uses out-of-place audio processing.",
            ),
            Self::ProcessAudioBasicInPlace => String::from(
                "Processes random audio through the plugin with its default parameter values and tests whether the \
                 output does not contain any non-finite or subnormal values. Uses in-place audio processing for buses \
                 that support it.",
            ),
            Self::ProcessAudioDoubleOutOfPlace => format!(
                "Same as '{}', but uses 64-bit floating point audio buffers instead of 32-bit ones for ports that \
                 support it.",
                Self::ProcessAudioBasicOutOfPlace,
            ),
            Self::ProcessAudioDoubleInPlace => format!(
                "Same as '{}', but uses 64-bit floating point audio buffers instead of 32-bit ones for ports that \
                 support it.",
                Self::ProcessAudioBasicInPlace,
            ),
            Self::ProcessAudioDenormals => String::from(
                "Processes random audio through the plugin with its default parameter values two times: without and \
                 with denormals as the input. Emits a warning if processing denormals causes a significant slowdown.",
            ),
            Self::LayoutAudioPortsActivation => format!(
                "Same as '{}', but this time it toggles the activation state of audio ports on and off via the \
                 'audio-ports-activation' extension.",
                Self::ProcessAudioBasicOutOfPlace,
            ),
            Self::LayoutConfigurableAudioPorts => format!(
                "Same as '{}', but this time it tries random configurations exposed via the \
                 'configurable-audio-ports' extension.",
                Self::ProcessAudioBasicOutOfPlace,
            ),
            Self::LayoutAudioPortsConfig => format!(
                "Same as '{}', but this time it tries all available port configurations exposed via the \
                 'audio-ports-config' extension.",
                Self::ProcessAudioBasicInPlace,
            ),
            Self::ProcessSleepConstantMask => String::from(
                "Processes random audio through the plugin with its default parameter values while setting the \
                 constant mask on silent blocks, and tests whether the output does not contain any non-finite or \
                 subnormal values and that the plugin sets the constant mask correctly",
            ),
            Self::ProcessSleepProcessStatus => String::from(
                "Processes random audio through the plugin with its default parameter values while checking if the \
                 output is consistent with the returned process status, and tests whether the output does not contain \
                 any non-finite or subnormal values and that the plugin sets the process status correctly",
            ),
            Self::ProcessNoteOutOfPlaceBasic => String::from(
                "Sends audio and random note and MIDI events to the plugin with its default parameter values and \
                 tests the output for consistency. Uses out-of-place audio processing.",
            ),
            Self::ProcessNoteInconsistent => String::from(
                "Sends intentionally inconsistent and mismatching note and MIDI events to the plugin with its default \
                 parameter values and tests the output for consistency. Uses out-of-place audio processing.",
            ),
            Self::ProcessNoteWildcard => format!(
                "Same as {}, but this time some note events have their note ID, port index, channel, or key set to \
                 -1, which means they can match multiple notes at the same time. This tests whether the plugin can \
                 handle such wildcard events without crashing or producing invalid output. Uses out-of-place audio \
                 processing.",
                Self::ProcessNoteOutOfPlaceBasic
            ),
            Self::ProcessVaryingSampleRates => String::from(
                "Processes random audio and random note events through the plugin with its default parameter values \
                 while trying different sample rates ranging from 1kHz to 768kHz, including fractional rates, and \
                 tests whether the output does not contain any non-finite or subnormal values. Uses out-of-place \
                 audio processing.",
            ),
            Self::ProcessVaryingBlockSizes => String::from(
                "Processes random audio and random note events through the plugin with its default parameter values \
                 while trying different maximum block sizes ranging from 1 to 16k, including non-power-of-two ones, \
                 and tests whether the output does not contain any non-finite or subnormal values. Uses out-of-place \
                 audio processing.",
            ),
            Self::ProcessRandomBlockSizes => String::from(
                "Processes random audio and random note events through the plugin with maximum block size of 2048 \
                 while randomizing block sizes for each process call, and tests whether the output does not contain \
                 any non-finite or subnormal values. Uses out-of-place audio processing.",
            ),
            Self::ProcessResetReactivate => String::from(
                "Asserts that resetting the plugin via 'clap_plugin::reset()' and via re-activation does not cause \
                 any crashes, and that the plugin still produces valid (non-NaN and non-infinite) output",
            ),
            Self::ParamConversions => String::from(
                "Asserts that value to string and string to value conversions are supported for ether all or none of \
                 the plugin's parameters, and that conversions between values and strings roundtrip consistently.",
            ),
            Self::ParamSetEvents => String::from(
                "Asserts that the resulting parameter values after a flush are the same as if the parameter changes \
                 were sent via a process call.",
            ),
            Self::ParamSetNoCookies => format!(
                "Same as '{}', but this time the parameter change events are sent with null cookies. The plugin \
                 should behave identically to when the cookies are set to non-null values.",
                Self::ParamSetEvents,
            ),
            Self::ParamFuzzBasic => format!(
                "Generates {} sets of random parameter values, sets those on the plugin, and has the plugin process \
                 {} buffers of random audio and note events. The plugin passes the test if it doesn't produce any \
                 infinite or NaN values, and doesn't crash.",
                params::FUZZ_NUM_PERMUTATIONS,
                params::FUZZ_RUNS_PER_PERMUTATION
            ),
            Self::ParamFuzzBounds => format!(
                "The exact same test as '{}', but this time the parameter values are snapped to the minimum and \
                 maximum values.",
                Self::ParamFuzzBasic
            ),
            Self::ParamFuzzSampleAccurate => String::from(
                "Sets parameter values in a sample-accurate fashion while processing audio, generating them at fixed \
                 intervals (10, 100, 1000 samples). The plugin passes the test if it doesn't produce any infinite or \
                 NaN values, and doesn't crash.",
            ),
            Self::ParamFuzzModulation => String::from(
                "Sends parameter change events, including monophonic modulation and polyphonic automation/modulation \
                 events at random irregular unsynchronized intervals, and have the plugin process them. The plugin \
                 passes the test if it doesn't produce any infinite or NaN values, and doesn't crash.",
            ),
            Self::ParamSetWrongNamespace => String::from(
                "Sends events to the plugin with the 'CLAP_EVENT_PARAM_VALUE' event type but with a mismatching \
                 namespace ID. Asserts that the plugin's parameter values don't change.",
            ),
            Self::ParamDefaultValues => String::from(
                "Asserts that the values for all parameters are set correctly to their default values when the plugin \
                 is initialized.",
            ),
            Self::StateInvalidEmpty => String::from(
                "The plugin should return false when 'clap_plugin_state::load()' is called with an empty state.",
            ),
            Self::StateInvalidRandom => String::from(
                "Loads 3x1MB chunks of random bytes via 'clap_plugin_state::load()' and asserts that the plugin \
                 doesn't crash.",
            ),
            Self::StateReproducibilityBasic => String::from(
                "Randomizes a plugin's parameters, saves its state, recreates the plugin instance, reloads the state, \
                 and then checks whether the parameter values are the same and whether saving the state once more \
                 results in the same parameters as before. The parameter values are updated using the process \
                 function.",
            ),
            Self::StateReproducibilityBuffered => format!(
                "Performs the same parameter reproducibility check as in '{}', but this time the plugin is only \
                 allowed to read a small prime number of bytes at a time when reloading and resaving the state.",
                Self::StateReproducibilityBasic
            ),
            Self::StateReproducibilityBinary => format!(
                "Performs the same parameter reproducibility check as in '{}', but also checks that the saved state \
                 data is exactly the same byte for byte. This means that the plugin needs to save the state in a \
                 completely deterministic way, without any non-determinism coming from things like uninitialized \
                 memory or random bytes.",
                Self::StateReproducibilityBasic
            ),
            Self::TransportNull => String::from(
                "Performs audio processing with a 'null' transport pointer, simulating a free-running transport \
                 state. The plugin passes the test if it doesn't produce any infinite or NaN values, and doesn't \
                 crash.",
            ),
            Self::TransportFuzz => String::from(
                "Performs audio processing while randomly changing the transport state on every block. The plugin \
                 passes the test if it doesn't produce any infinite or NaN values, and doesn't crash.",
            ),
            Self::TransportFuzzSampleAccurate => format!(
                "Same as '{}', but this time the test sends 'clap_event_transport' events in sample-accurate fashion \
                 while processing audio, generating them at fixed intervals (1, 100, 1000 samples). The plugin passes \
                 the test if it doesn't produce any infinite or NaN values, and doesn't crash.",
                Self::TransportFuzz
            ),
        }
    }

    pub fn run(&self, library_path: &Path, plugin_id: &str) -> Result<TestStatus> {
        let _span = Span::begin(
            self.into(),
            record! {
                library_path: library_path.display().to_string(),
                plugin_id: plugin_id
            },
        );

        // SAFETY: This is called on the main thread.
        let library = &PluginLibrary::load(library_path)
            .with_context(|| format!("Could not load '{}'", library_path.display()))?;

        match self {
            Self::DescriptorConsistency => descriptor::test_consistency(library, plugin_id),
            Self::FeaturesCategories => descriptor::test_features_categories(library, plugin_id),
            Self::FeaturesDuplicates => descriptor::test_features_duplicates(library, plugin_id),
            Self::LayoutAudioPortsActivation => layout::test_layout_audio_ports_activation(library, plugin_id),
            Self::LayoutAudioPortsConfig => layout::test_layout_audio_ports_config(library, plugin_id),
            Self::LayoutConfigurableAudioPorts => layout::test_layout_configurable_audio_ports(library, plugin_id),
            Self::ProcessAudioBasicOutOfPlace => processing::test_process_audio_basic(library, plugin_id, false),
            Self::ProcessAudioBasicInPlace => processing::test_process_audio_basic(library, plugin_id, true),
            Self::ProcessAudioDoubleOutOfPlace => processing::test_process_audio_double(library, plugin_id, false),
            Self::ProcessAudioDoubleInPlace => processing::test_process_audio_double(library, plugin_id, true),
            Self::ProcessAudioDenormals => processing::test_process_audio_denormals(library, plugin_id),
            Self::ProcessSleepConstantMask => processing::test_process_sleep_constant_mask(library, plugin_id),
            Self::ProcessSleepProcessStatus => processing::test_process_sleep_process_status(library, plugin_id),
            Self::ProcessNoteOutOfPlaceBasic => {
                processing::test_process_note_out_of_place(library, plugin_id, false, false)
            }
            Self::ProcessNoteInconsistent => {
                processing::test_process_note_out_of_place(library, plugin_id, true, false)
            }
            Self::ProcessNoteWildcard => processing::test_process_note_out_of_place(library, plugin_id, false, true),
            Self::ProcessVaryingSampleRates => processing::test_process_varying_sample_rates(library, plugin_id),
            Self::ProcessVaryingBlockSizes => processing::test_process_varying_block_sizes(library, plugin_id),
            Self::ProcessRandomBlockSizes => processing::test_process_random_block_sizes(library, plugin_id),
            Self::ProcessResetReactivate => processing::test_process_reset_reactivate(library, plugin_id),
            Self::ParamConversions => params::test_param_conversions(library, plugin_id),
            Self::ParamSetEvents => params::test_param_set_events(library, plugin_id, false),
            Self::ParamSetNoCookies => params::test_param_set_events(library, plugin_id, true),
            Self::ParamSetWrongNamespace => params::test_param_set_wrong_namespace(library, plugin_id),
            Self::ParamFuzzBasic => params::test_param_fuzz_basic(library, plugin_id, false),
            Self::ParamFuzzBounds => params::test_param_fuzz_basic(library, plugin_id, true),
            Self::ParamFuzzSampleAccurate => params::test_param_fuzz_sample_accurate(library, plugin_id),
            Self::ParamFuzzModulation => params::test_param_fuzz_modulation(library, plugin_id),
            Self::ParamDefaultValues => params::test_param_default_values(library, plugin_id),
            Self::StateInvalidEmpty => state::test_state_invalid_empty(library, plugin_id),
            Self::StateInvalidRandom => state::test_state_invalid_random(library, plugin_id),
            Self::StateReproducibilityBasic => state::test_state_reproducibility(library, plugin_id, false, false),
            Self::StateReproducibilityBuffered => state::test_state_reproducibility(library, plugin_id, true, false),
            Self::StateReproducibilityBinary => state::test_state_reproducibility(library, plugin_id, false, true),
            Self::TransportNull => transport::test_transport_null(library, plugin_id),
            Self::TransportFuzz => transport::test_transport_fuzz(library, plugin_id),
            Self::TransportFuzzSampleAccurate => transport::test_transport_fuzz_sample_accurate(library, plugin_id),
        }
    }
}
