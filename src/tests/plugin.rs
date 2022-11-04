//! Tests for individual plugin instances.

use clap::ValueEnum;
use std::process::Command;

use super::{TestCase, TestResult};
use crate::plugin::library::PluginLibrary;

mod features;
mod params;
mod processing;
mod state;

const TEST_CATEGORY_FEATURES: &str = "features-categories";
const TEST_DUPLICATE_FEATURES: &str = "features-duplicates";
const TEST_BASIC_OUT_OF_PLACE_AUDIO_PROCESSING: &str = "process-audio-out-of-place-basic";
const TEST_BASIC_OUT_OF_PLACE_NOTE_PROCESSING: &str = "process-note-out-of-place-basic";
const TEST_INCONSISTENT_NOTE_PROCESSING: &str = "process-note-inconsistent";
const TEST_CONVERT_PARAMS: &str = "param-conversions";
const TEST_WRONG_NAMESPACE_SET_PARAMS: &str = "param-set-wrong-namespace";
const TEST_BASIC_STATE_REPRODUCIBILITY: &str = "state-reproducibility-basic";
const TEST_NULL_COOKIES_STATE_REPRODUCIBILITY: &str = "state-reproducibility-null-cookies";
const TEST_FLUSH_STATE_REPRODUCIBILITY: &str = "state-reproducibility-flush";
const TEST_BUFFERED_STATE_STREAMS: &str = "state-buffered-streams";

/// The tests for individual CLAP plugins. See the module's heading for more information, and the
/// `description` function below for a description of each test case.
pub enum PluginTestCase {
    CategoryFeatures,
    DuplicateFeatures,
    BasicOutOfPlaceAudioProcessing,
    BasicOutOfPlaceNoteProcessing,
    InconsistentNoteProcessing,
    ConvertParams,
    WrongNamespaceSetParams,
    BasicStateReproducibility,
    NullCookiesStateReproducibility,
    FlushStateReproducibility,
    BufferedStateStreams,
}

impl<'a> TestCase<'a> for PluginTestCase {
    /// A loaded CLAP plugin library and the ID of the plugin contained within that library that
    /// should be tested.
    type TestArgs = (&'a PluginLibrary, &'a str);

    const ALL: &'static [Self] = &[
        PluginTestCase::CategoryFeatures,
        PluginTestCase::DuplicateFeatures,
        PluginTestCase::BasicOutOfPlaceAudioProcessing,
        PluginTestCase::BasicOutOfPlaceNoteProcessing,
        PluginTestCase::InconsistentNoteProcessing,
        PluginTestCase::ConvertParams,
        PluginTestCase::WrongNamespaceSetParams,
        PluginTestCase::BasicStateReproducibility,
        PluginTestCase::NullCookiesStateReproducibility,
        PluginTestCase::FlushStateReproducibility,
        PluginTestCase::BufferedStateStreams,
    ];

    fn from_str(string: &str) -> Option<Self> {
        match string {
            TEST_CATEGORY_FEATURES => Some(PluginTestCase::CategoryFeatures),
            TEST_DUPLICATE_FEATURES => Some(PluginTestCase::DuplicateFeatures),
            TEST_BASIC_OUT_OF_PLACE_AUDIO_PROCESSING => {
                Some(PluginTestCase::BasicOutOfPlaceAudioProcessing)
            }
            TEST_BASIC_OUT_OF_PLACE_NOTE_PROCESSING => {
                Some(PluginTestCase::BasicOutOfPlaceNoteProcessing)
            }
            TEST_INCONSISTENT_NOTE_PROCESSING => Some(PluginTestCase::InconsistentNoteProcessing),
            TEST_CONVERT_PARAMS => Some(PluginTestCase::ConvertParams),
            TEST_WRONG_NAMESPACE_SET_PARAMS => Some(PluginTestCase::WrongNamespaceSetParams),
            TEST_BASIC_STATE_REPRODUCIBILITY => Some(PluginTestCase::BasicStateReproducibility),
            TEST_NULL_COOKIES_STATE_REPRODUCIBILITY => {
                Some(PluginTestCase::NullCookiesStateReproducibility)
            }
            TEST_FLUSH_STATE_REPRODUCIBILITY => Some(PluginTestCase::FlushStateReproducibility),
            TEST_BUFFERED_STATE_STREAMS => Some(PluginTestCase::BufferedStateStreams),
            _ => None,
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            PluginTestCase::CategoryFeatures => TEST_CATEGORY_FEATURES,
            PluginTestCase::DuplicateFeatures => TEST_DUPLICATE_FEATURES,
            PluginTestCase::BasicOutOfPlaceAudioProcessing => {
                TEST_BASIC_OUT_OF_PLACE_AUDIO_PROCESSING
            }
            PluginTestCase::BasicOutOfPlaceNoteProcessing => {
                TEST_BASIC_OUT_OF_PLACE_NOTE_PROCESSING
            }
            PluginTestCase::InconsistentNoteProcessing => TEST_INCONSISTENT_NOTE_PROCESSING,
            PluginTestCase::ConvertParams => TEST_CONVERT_PARAMS,
            PluginTestCase::WrongNamespaceSetParams => TEST_WRONG_NAMESPACE_SET_PARAMS,
            PluginTestCase::BasicStateReproducibility => TEST_BASIC_STATE_REPRODUCIBILITY,
            PluginTestCase::NullCookiesStateReproducibility => {
                TEST_NULL_COOKIES_STATE_REPRODUCIBILITY
            }
            PluginTestCase::FlushStateReproducibility => TEST_FLUSH_STATE_REPRODUCIBILITY,
            PluginTestCase::BufferedStateStreams => TEST_BUFFERED_STATE_STREAMS,
        }
    }

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
                "The exact same test as {TEST_BASIC_STATE_REPRODUCIBILITY}, but with all cookies \
                 in the parameter events set to null pointers. The plugin should handle this in \
                 the same way as the other test case.",
            ),
            PluginTestCase::FlushStateReproducibility => String::from(
                "Randomizes a plugin's parameters, saves its state, recreates the plugin \
                 instance, sets the same parameters as before, saves the state again, and then \
                 asserts that the two states are identical. The parameter values are set updated \
                 using the process function to create the first state, and using the flush \
                 function to create the second state.",
            ),
            PluginTestCase::BufferedStateStreams => format!(
                "Performs the same state and parameter reproducibility check as in \
                 '{TEST_BASIC_STATE_REPRODUCIBILITY}', but this time the plugin is only allowed \
                 to read a small prime number of bytes at a time when reloading and resaving the \
                 state."
            ),
        }
    }

    fn set_out_of_process_args(&self, command: &mut Command, (library, plugin_id): Self::TestArgs) {
        let test_name = self.as_str();

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
