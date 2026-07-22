//! Panic handling utilities.
//!
//! When testing, validator should return an `Error` if the plugin is misbehaving, not panic.
//! Any panics (except for [`fail_test!`]) while testing are considered a bug in the validator itself, making them worth logging.

pub fn install_panic_hook() {
    #[track_caller]
    fn hook(info: &std::panic::PanicHookInfo) {
        let backtrace = std::backtrace::Backtrace::capture();
        let backtrace = if backtrace.status() == std::backtrace::BacktraceStatus::Disabled {
            String::from(". Set RUST_BACKTRACE=1 for a backtrace.")
        } else {
            format!("\n{}", backtrace)
        };

        let thread = std::thread::current().name().unwrap_or("<unnamed>").to_owned();
        let message = panic_message(info.payload());

        match info.location() {
            Some(location) => {
                log::error!(
                    target: "panic", "thread '{}' panicked at '{}': {}:{}{}",
                    thread,
                    message,
                    location.file(),
                    location.line(),
                    backtrace
                );
            }
            None => log::error!(
                target: "panic",
                "thread '{}' panicked at '{}'{:?}",
                thread,
                message,
                backtrace
            ),
        }
    }

    std::panic::set_hook(Box::new(hook));
}

pub fn panic_message(panic: &dyn std::any::Any) -> String {
    if let Some(s) = panic.downcast_ref::<&'static str>() {
        format!("{}. This is a bug in the validator", s)
    } else if let Some(s) = panic.downcast_ref::<String>() {
        format!("{}. This is a bug in the validator", s)
    } else if let Some(message) = panic.downcast_ref::<TestFailure>() {
        message.0.clone()
    } else {
        "A panic occurred. This is a bug in the validator".to_string()
    }
}

#[doc(hidden)]
pub struct TestFailure(pub String);

/// Fails the current test with a panic, taking down the whole process.
/// This is a last-resort mechanism for when a test cannot continue due to an error in the plugin being tested.
/// Prefer regular error handling where possible.
///
/// The difference between this and a regular panic is that regular panics are treated as bugs in the validator itself.
macro_rules! fail_test {
    ($($arg:tt)*) => {
        std::panic::panic_any($crate::cli::TestFailure(format!($($arg)*)))
    };
}

pub(crate) use fail_test;
