//! Tests for individual plugin instances.

use std::process::Command;

use anyhow::Context;
use clap::ValueEnum;

use super::{TestCase, TestResult, TestStatus};
use crate::hosting::ClapHost;
use crate::plugin::library::ClapPluginLibrary;

/// The string representation for [`PluginTestCase::BasicAudioProcessing`].
const BASIC_AUDIO_PROCESSING: &str = "process-basic";

/// The tests for individual CLAP plugins. See the module's heading for more information.
pub enum PluginTestCase {
    /// Sends audio and MIDI to the plugin (depending on what it supports) with the initial
    /// parmaeters, and asserts that the audio output does not contain any non-finite or subnormal
    /// values.
    BasicAudioProcessing,
}

impl<'a> TestCase<'a> for PluginTestCase {
    /// A loaded CLAP plugin library and the ID of the plugin contained within that library that
    /// should be tested.
    type TestArgs = (&'a ClapPluginLibrary, &'a str);

    const ALL: &'static [Self] = &[PluginTestCase::BasicAudioProcessing];

    fn from_str(string: &str) -> Option<Self> {
        match string {
            BASIC_AUDIO_PROCESSING => Some(PluginTestCase::BasicAudioProcessing),
            _ => None,
        }
    }

    fn as_str(&self) -> &'static str {
        match &self {
            PluginTestCase::BasicAudioProcessing => BASIC_AUDIO_PROCESSING,
        }
    }

    fn description(&self) -> String {
        match &self {
            PluginTestCase::BasicAudioProcessing => String::from("Sends random audio and/or MIDI through the plugin with its default parameter values and tests whether the output does not contain any non-finite or subnormal values."),
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
        let result = match &self {
            PluginTestCase::BasicAudioProcessing => {
                // The host doesn't need to do anything special for this test
                let host = ClapHost::new();
                let plugin = library
                    .create_plugin(plugin_id, host)
                    .context("Could not create the plugin instance");

                // TODO: Query the audio and note ports with their default configuration
                // TODO: Spawn an audio thread
                // TODO: Process audio in the audio thread and check the output

                match plugin {
                    // Ok(_) => TestStatus::Success { notes: None },
                    Ok(_) => TestStatus::Skipped {
                        reason: Some(String::from("Not yet implemented")),
                    },
                    Err(err) => TestStatus::Failed {
                        reason: Some(format!("{err:#}")),
                    },
                }
            }
        };

        self.create_result(result)
    }
}
