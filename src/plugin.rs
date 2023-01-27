//! Contains functions for loading and interacting with CLAP plugins.

pub mod ext;
pub mod instance;
pub mod library;

/// Used for asserting that the plugin is in the correct state when calling a function. Hard panics
/// if this is not the case. This is used to ensure the validator's correctness.
///
/// Requires a `.status()` method to exist on `$self`.
macro_rules! assert_plugin_state_eq {
    ($self:expr, $expected:expr) => {
        let status = $self.status();
        if status != $expected {
            panic!(
                "Invalid plugin function call while the plugin is in an incorrect state ({:?} != \
                 {:?}). This is a bug in the validator.",
                status, $expected
            )
        }
    };
}

/// Used for asserting that the plugin is a lower state then the specified one before calling a
/// function. Hard panics if this is not the case. This is used to ensure the validator's
/// correctness.
///
/// Requires a `.status()` method to exist on `$self`.
macro_rules! assert_plugin_state_lt {
    ($self:expr, $other:expr) => {
        let status = $self.status();
        if status >= $other {
            panic!(
                "Invalid plugin function call while the plugin is in an incorrect state ({:?} >= \
                 {:?}). This is a bug in the validator.",
                status, $other
            )
        }
    };
}

/// Used for asserting that the plugin has been initialized. Hard panics if this is not the case.
/// This is used to ensure the validator's correctness.
///
/// Requires a `.status()` method to exist on `$self`.
macro_rules! assert_plugin_state_initialized {
    ($self:expr) => {
        let status = $self.status();
        if status == PluginStatus::Uninitialized {
            panic!(
                "Invalid plugin function call while the plugin has not yet been initialized. This \
                 is a bug in the validator."
            )
        }
    };
}

pub(crate) use assert_plugin_state_eq;
pub(crate) use assert_plugin_state_initialized;
pub(crate) use assert_plugin_state_lt;
