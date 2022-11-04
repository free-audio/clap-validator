//! Tests for individual plugin instances.

use clap::ValueEnum;
use std::process::Command;

use super::{TestCase, TestResult};
use crate::plugin::library::PluginLibrary;

mod features;
mod params;
mod processing;
mod state;

/// The tests for individual CLAP plugins. See the module's heading for more information, and the
/// `description` function below for a description of each test case.
#[derive(strum_macros::Display, strum_macros::EnumString, strum_macros::EnumIter)]
pub enum PluginTestCase {
    #[strum(serialize = "features-categories")]
    CategoryFeatures,
    #[strum(serialize = "features-duplicates")]
    DuplicateFeatures,
    #[strum(serialize = "process-audio-out-of-place-basic")]
    BasicOutOfPlaceAudioProcessing,
    #[strum(serialize = "process-note-out-of-place-basic")]
    BasicOutOfPlaceNoteProcessing,
    #[strum(serialize = "process-note-inconsistent")]
    InconsistentNoteProcessing,
    #[strum(serialize = "param-conversions")]
    ConvertParams,
    #[strum(serialize = "param-set-wrong-namespace")]
    WrongNamespaceSetParams,
    #[strum(serialize = "state-reproducibility-basic")]
    BasicStateReproducibility,
    #[strum(serialize = "state-reproducibility-null-cookies")]
    NullCookiesStateReproducibility,
    #[strum(serialize = "state-reproducibility-flush")]
    FlushStateReproducibility,
    #[strum(serialize = "state-buffered-streams")]
    BufferedStateStreams,
}

impl<'a> TestCase<'a> for PluginTestCase {
    /// A loaded CLAP plugin library and the ID of the plugin contained within that library that
    /// should be tested.
    type TestArgs = (&'a PluginLibrary, &'a str);

    fn description(&self) -> String {
        match self {
            PluginTestCase::CategoryFeatures => String::from(
                "The plugin needs to have at least one of the main CLAP category features.",
            ),
            PluginTestCase::DuplicateFeatures => {
                String::from("The plugin's features array should not contain any duplicates.")
            }
            PluginTestCase::BasicOutOfPlaceAudioProcessing => String::from(
                "Processes random audio through the plugin with its default parameter values and \
                 tests whether the output does not contain any non-finite or subnormal values. \
                 Uses out-of-place audio processing.",
            ),
            PluginTestCase::BasicOutOfPlaceNoteProcessing => String::from(
                "Sends audio and random note and MIDI events to the plugin with its default \
                 parameter values and tests the output for consistency. Uses out-of-place audio \
                 processing.",
            ),
            PluginTestCase::InconsistentNoteProcessing => String::from(
                "Sends intentionally inconsistent and mismatching note and MIDI events to the \
                 plugin with its default parameter values and tests the output for consistency. \
                 Uses out-of-place audio processing.",
            ),
            PluginTestCase::ConvertParams => String::from(
                "Asserts that value to string and string to value conversions are supported for \
                 ether all or none of the plugin's parameters, and that conversions between \
                 values and strings roundtrip consistently.",
            ),
            PluginTestCase::WrongNamespaceSetParams => String::from(
                "Sends events to the plugin with the 'CLAP_EVENT_PARAM_VALUE' event tyep but with \
                 a mismatching namespace ID. Asserts that the plugin's parameter values don't \
                 change.",
            ),
            PluginTestCase::BasicStateReproducibility => String::from(
                "Randomizes a plugin's parameters, saves its state, recreates the plugin \
                 instance, reloads the state, and then checks whether the parameter values are \
                 the same and whether saving the state once more results in the same state file \
                 as before. The parameter values are updated using the process function.",
            ),
            PluginTestCase::NullCookiesStateReproducibility => format!(
                "The exact same test as {}, but with all cookies in the parameter events set to \
                 null pointers. The plugin should handle this in the same way as the other test \
                 case.",
                PluginTestCase::BasicStateReproducibility
            ),
            PluginTestCase::FlushStateReproducibility => String::from(
                "Randomizes a plugin's parameters, saves its state, recreates the plugin \
                 instance, sets the same parameters as before, saves the state again, and then \
                 asserts that the two states are identical. The parameter values are set updated \
                 using the process function to create the first state, and using the flush \
                 function to create the second state.",
            ),
            PluginTestCase::BufferedStateStreams => format!(
                "Performs the same state and parameter reproducibility check as in '{}', but this \
                 time the plugin is only allowed to read a small prime number of bytes at a time \
                 when reloading and resaving the state.",
                PluginTestCase::BasicStateReproducibility
            ),
        }
    }

    fn set_out_of_process_args(&self, command: &mut Command, (library, plugin_id): Self::TestArgs) {
        let test_name = self.to_string();

        command
            .arg(
                crate::validator::SingleTestType::Plugin
                    .to_possible_value()
                    .unwrap()
                    .get_name(),
            )
            .arg(library.library_path())
            .arg(plugin_id)
            .arg(test_name);
    }

    fn run_in_process(&self, (library, plugin_id): Self::TestArgs) -> TestResult {
        let status = match self {
            PluginTestCase::CategoryFeatures => {
                features::test_category_features(library, plugin_id)
            }
            PluginTestCase::DuplicateFeatures => {
                features::test_duplicate_features(library, plugin_id)
            }
            PluginTestCase::BasicOutOfPlaceAudioProcessing => {
                processing::test_basic_out_of_place_audio_processing(library, plugin_id)
            }
            PluginTestCase::BasicOutOfPlaceNoteProcessing => {
                processing::test_basic_out_of_place_note_processing(library, plugin_id)
            }
            PluginTestCase::InconsistentNoteProcessing => {
                processing::test_inconsistent_note_processing(library, plugin_id)
            }
            PluginTestCase::ConvertParams => params::test_convert_params(library, plugin_id),
            PluginTestCase::WrongNamespaceSetParams => {
                params::test_wrong_namespace_set_params(library, plugin_id)
            }
            PluginTestCase::BasicStateReproducibility => {
                state::test_basic_state_reproducibility(library, plugin_id, false)
            }
            PluginTestCase::NullCookiesStateReproducibility => {
                state::test_basic_state_reproducibility(library, plugin_id, true)
            }
            PluginTestCase::FlushStateReproducibility => {
                state::test_flush_state_reproducibility(library, plugin_id)
            }
            PluginTestCase::BufferedStateStreams => {
                state::test_buffered_state_streams(library, plugin_id)
            }
        };

        self.create_result(status)
    }
}
