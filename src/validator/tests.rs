//! All tests in the validation test suite.
//!
//! The results for the tests need to be serializable as JSON, and there also needs to be some way
//! to refer to a single test in a cli invocation (in order to be able to run tests out of process).
//! To facilitate this, the test cases are all identified by variants in an enum, and that enum can
//! be converted to and from a string representation.

use std::time::Duration;

/// The string representation for [`TestCase::PluginScanTime`].
pub const TEST_PLUGIN_SCAN_TIME: &str = "plugin-scan-time";

pub const PLUGIN_SCAN_TIME_LIMIT: Duration = Duration::from_millis(100);

/// All test in the validator. See the module's heading for more information.
pub enum TestCase {
    /// Asserts whether the plugin takes longer than `PLUGIN_SCAN_TIME_LIMIT` to scan.
    PluginScanTime,
}

impl TestCase {
    /// All available test cases.
    const ALL: [TestCase; 1] = [TestCase::PluginScanTime];

    /// Try to parse a test case's string representation as produced by
    /// [`as_str()`][Self::as_str()]. Returns `None` if the test case name was not recognized.
    pub fn from_str(string: &str) -> Option<Self> {
        match string {
            TEST_PLUGIN_SCAN_TIME => Some(TestCase::PluginScanTime),
            _ => None,
        }
    }

    /// Get the string representation of this test case.
    pub fn as_str(&self) -> &'static str {
        match self {
            TestCase::PluginScanTime => TEST_PLUGIN_SCAN_TIME,
        }
    }

    /// Get the textual description for a test case. This description won't contain any line breaks,
    /// but it may consist of multiple sentences.
    pub fn description(&self) -> String {
        match self {
            TestCase::PluginScanTime => format!(
                "Tests whether the plugin can be scanned in under {} milliseconds.",
                PLUGIN_SCAN_TIME_LIMIT.as_millis()
            ),
        }
    }
}
