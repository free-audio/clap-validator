//! Tests for individual plugin instances.

use clap::ValueEnum;
use std::path::Path;
use std::process::Command;

use super::{TestCase, TestResult};
use crate::plugin::library::PluginLibrary;

mod descriptor;
mod params;
mod processing;
mod state;

pub use processing::ProcessingTest;

/// The tests for individual CLAP plugins. See the module's heading for more information, and the
/// `description` function below for a description of each test case.
#[derive(strum_macros::Display, strum_macros::EnumString, strum_macros::EnumIter)]
pub enum PluginTestCase {
    #[strum(serialize = "descriptor-consistency")]
    DescriptorConsistency,
    #[strum(serialize = "features-categories")]
    FeaturesCategories,
    #[strum(serialize = "features-duplicates")]
    FeaturesDuplicates,
    #[strum(serialize = "process-audio-out-of-place-basic")]
    ProcessAudioOutOfPlaceBasic,
    #[strum(serialize = "process-note-out-of-place-basic")]
    ProcessNoteOutOfPlaceBasic,
    #[strum(serialize = "process-note-inconsistent")]
    ProcessNoteInconsistent,
    #[strum(serialize = "param-conversions")]
    ParamConversions,
    #[strum(serialize = "param-fuzz-basic")]
    ParamFuzzBasic,
    #[strum(serialize = "param-set-wrong-namespace")]
    ParamSetWrongNamespace,
    #[strum(serialize = "state-invalid")]
    StateInvalid,
    #[strum(serialize = "state-reproducibility-basic")]
    StateReproducibilityBasic,
    #[strum(serialize = "state-reproducibility-null-cookies")]
    StateReproducibilityNullCookies,
    #[strum(serialize = "state-reproducibility-flush")]
    StateReproducibilityFlush,
    #[strum(serialize = "state-buffered-streams")]
    StateBufferedStreams,
}

impl<'a> TestCase<'a> for PluginTestCase {
    /// Path to a CLAP plugin library, a loaded CLAP plugin library and the ID of the plugin contained
    /// within that library that should be tested.
    type TestArgs = (&'a Path, &'a PluginLibrary, &'a str);

    fn description(&self) -> String {
        match self {
            PluginTestCase::DescriptorConsistency => String::from(
                "The plugin descriptor returned from the plugin factory and the plugin descriptor \
                 stored on the 'clap_plugin object should be equivalent.",
            ),
            PluginTestCase::FeaturesCategories => String::from(
                "The plugin needs to have at least one of the main CLAP category features.",
            ),
            PluginTestCase::FeaturesDuplicates => {
                String::from("The plugin's features array should not contain any duplicates.")
            }
            PluginTestCase::ProcessAudioOutOfPlaceBasic => String::from(
                "Processes random audio through the plugin with its default parameter values and \
                 tests whether the output does not contain any non-finite or subnormal values. \
                 Uses out-of-place audio processing.",
            ),
            PluginTestCase::ProcessNoteOutOfPlaceBasic => String::from(
                "Sends audio and random note and MIDI events to the plugin with its default \
                 parameter values and tests the output for consistency. Uses out-of-place audio \
                 processing.",
            ),
            PluginTestCase::ProcessNoteInconsistent => String::from(
                "Sends intentionally inconsistent and mismatching note and MIDI events to the \
                 plugin with its default parameter values and tests the output for consistency. \
                 Uses out-of-place audio processing.",
            ),
            PluginTestCase::ParamConversions => String::from(
                "Asserts that value to string and string to value conversions are supported for \
                 ether all or none of the plugin's parameters, and that conversions between \
                 values and strings roundtrip consistently.",
            ),
            PluginTestCase::ParamFuzzBasic => format!(
                "Generates {} sets of random parameter values, sets those on the plugin, and has \
                 the plugin process {} buffers of random audio and note events. The plugin passes \
                 the test if it doesn't produce any infinite or NaN values, and doesn't crash.",
                params::FUZZ_NUM_PERMUTATIONS,
                params::FUZZ_RUNS_PER_PERMUTATION
            ),
            PluginTestCase::ParamSetWrongNamespace => String::from(
                "Sends events to the plugin with the 'CLAP_EVENT_PARAM_VALUE' event tyep but with \
                 a mismatching namespace ID. Asserts that the plugin's parameter values don't \
                 change.",
            ),
            PluginTestCase::StateInvalid => String::from(
                "The plugin should return false when 'clap_plugin_state::load()' is called with \
                 an empty state.",
            ),
            PluginTestCase::StateReproducibilityBasic => String::from(
                "Randomizes a plugin's parameters, saves its state, recreates the plugin \
                 instance, reloads the state, and then checks whether the parameter values are \
                 the same and whether saving the state once more results in the same state file \
                 as before. The parameter values are updated using the process function.",
            ),
            PluginTestCase::StateReproducibilityNullCookies => format!(
                "The exact same test as {}, but with all cookies in the parameter events set to \
                 null pointers. The plugin should handle this in the same way as the other test \
                 case.",
                PluginTestCase::StateReproducibilityBasic
            ),
            PluginTestCase::StateReproducibilityFlush => String::from(
                "Randomizes a plugin's parameters, saves its state, recreates the plugin \
                 instance, sets the same parameters as before, saves the state again, and then \
                 asserts that the two states are identical. The parameter values are set updated \
                 using the process function to create the first state, and using the flush \
                 function to create the second state.",
            ),
            PluginTestCase::StateBufferedStreams => format!(
                "Performs the same state and parameter reproducibility check as in '{}', but this \
                 time the plugin is only allowed to read a small prime number of bytes at a time \
                 when reloading and resaving the state.",
                PluginTestCase::StateReproducibilityBasic
            ),
        }
    }

    fn set_out_of_process_args(&self, command: &mut Command, (path, _library, plugin_id): Self::TestArgs) {
        let test_name = self.to_string();

        command
            .arg(
                crate::validator::SingleTestType::Plugin
                    .to_possible_value()
                    .unwrap()
                    .get_name(),
            )
            .arg(path)
            .arg(plugin_id)
            .arg(test_name);
    }

    fn run_in_process(&self, (_, library, plugin_id): Self::TestArgs) -> TestResult {
        let status = match self {
            PluginTestCase::DescriptorConsistency => {
                descriptor::test_consistency(library, plugin_id)
            }
            PluginTestCase::FeaturesCategories => {
                descriptor::test_features_categories(library, plugin_id)
            }
            PluginTestCase::FeaturesDuplicates => {
                descriptor::test_features_duplicates(library, plugin_id)
            }
            PluginTestCase::ProcessAudioOutOfPlaceBasic => {
                processing::test_process_audio_out_of_place_basic(library, plugin_id)
            }
            PluginTestCase::ProcessNoteOutOfPlaceBasic => {
                processing::test_process_note_out_of_place_basic(library, plugin_id)
            }
            PluginTestCase::ProcessNoteInconsistent => {
                processing::test_process_note_inconsistent(library, plugin_id)
            }
            PluginTestCase::ParamConversions => params::test_param_conversions(library, plugin_id),
            PluginTestCase::ParamFuzzBasic => params::test_param_fuzz_basic(library, plugin_id),
            PluginTestCase::ParamSetWrongNamespace => {
                params::test_param_set_wrong_namespace(library, plugin_id)
            }
            PluginTestCase::StateInvalid => state::test_state_invalid(library, plugin_id),
            PluginTestCase::StateReproducibilityBasic => {
                state::test_state_reproducibility_null_cookies(library, plugin_id, false)
            }
            PluginTestCase::StateReproducibilityNullCookies => {
                state::test_state_reproducibility_null_cookies(library, plugin_id, true)
            }
            PluginTestCase::StateReproducibilityFlush => {
                state::test_state_reproducibility_flush(library, plugin_id)
            }
            PluginTestCase::StateBufferedStreams => {
                state::test_state_buffered_streams(library, plugin_id)
            }
        };

        self.create_result(status)
    }
}
