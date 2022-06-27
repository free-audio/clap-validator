//! Tests for individual plugin instances.

use std::process::Command;

use anyhow::Context;
use clap::ValueEnum;

use super::{TestCase, TestResult, TestStatus};
use crate::hosting::ClapHost;
use crate::plugin::ext::audio_ports::{AudioPortConfig, AudioPorts};
use crate::plugin::ext::note_ports::{NotePortConfig, NotePorts};
use crate::plugin::library::PluginLibrary;

/// The string representation for [`PluginTestCase::BasicCombinedAudioProcessing`].
const BASIC_COMBINED_AUDIO_PROCESSING: &str = "process-combined-basic";

/// The tests for individual CLAP plugins. See the module's heading for more information.
pub enum PluginTestCase {
    /// Sends audio and MIDI to the plugin (depending on what it supports) with the initial
    /// parmaeters, and asserts that the audio output does not contain any non-finite or subnormal
    /// values.
    // TODO: Similar test cases that only do audio or only do MIDI
    BasicCombinedAudioProcessing,
}

impl<'a> TestCase<'a> for PluginTestCase {
    /// A loaded CLAP plugin library and the ID of the plugin contained within that library that
    /// should be tested.
    type TestArgs = (&'a PluginLibrary, &'a str);

    const ALL: &'static [Self] = &[PluginTestCase::BasicCombinedAudioProcessing];

    fn from_str(string: &str) -> Option<Self> {
        match string {
            BASIC_COMBINED_AUDIO_PROCESSING => Some(PluginTestCase::BasicCombinedAudioProcessing),
            _ => None,
        }
    }

    fn as_str(&self) -> &'static str {
        match &self {
            PluginTestCase::BasicCombinedAudioProcessing => BASIC_COMBINED_AUDIO_PROCESSING,
        }
    }

    fn description(&self) -> String {
        match &self {
            PluginTestCase::BasicCombinedAudioProcessing => String::from("Sends random audio and/or MIDI through the plugin with its default parameter values and tests whether the output does not contain any non-finite or subnormal values."),
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
            PluginTestCase::BasicCombinedAudioProcessing => {
                // The host doesn't need to do anything special for this test
                let host = ClapHost::new();
                let result = library
                    .create_plugin(plugin_id, host.clone())
                    .context("Could not create the plugin instance")
                    .and_then(|plugin| {
                        plugin.init().context("Error during initialization")?;

                        // Get the plugin's audio channel layout and note port configuration, if it
                        // supports those extensions
                        let audio_ports_config = match plugin.get_extension::<AudioPorts>() {
                            Some(audio_ports) => audio_ports
                                .config()
                                .context("Error while querying 'audio_ports' IO configuration")?,
                            None => AudioPortConfig::default(),
                        };

                        let note_port_config = match plugin.get_extension::<NotePorts>() {
                            Some(note_ports) => note_ports
                                .config()
                                .context("Error while querying 'note_ports' IO configuration")?,
                            None => NotePortConfig::default(),
                        };

                        Ok((plugin, audio_ports_config, note_port_config))
                    });

                // TODO: Spawn an audio thread
                // TODO: Process audio in the audio thread and check the output

                // The `ClapHost` also contains built-in thread safety checks
                let result = result.and_then(|_| {
                    host.thread_safety_check()
                        .context("Thread safety checks failed")
                });

                match result {
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
