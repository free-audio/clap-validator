//! All the different commands for the cli. Split up into modules and functions to make it a bit
//! easier to navigate.

use std::collections::HashMap;

pub mod list;
pub mod validate;

/// A helper for printing terminal wrapped and indentend strings to STDOUT.
pub struct TextWrapper {
    /// The basic wrapping options, minus the indent string.
    wrapping_options: textwrap::Options<'static>,
    /// Indent strings for different widths. Need to be allocated separately because textwrap
    /// doesn't let you directly indent to a certain number of spaces.
    indent_strings: HashMap<usize, String>,
}

impl Default for TextWrapper {
    fn default() -> Self {
        Self {
            wrapping_options: textwrap::Options::with_termwidth(),
            indent_strings: HashMap::new(),
        }
    }
}

/// Shorthand for `wrapper.print_auto(format!(...))`.
macro_rules! println_wrapped {
    ($wrapper:expr, $($arg:tt)*) => {
        $wrapper.print_auto(format!($($arg)*))
    }
}
pub(crate) use println_wrapped;

/// Shorthand for `wrapper.print_auto_no_indent(format!(...))`.
macro_rules! println_wrapped_no_indent {
    ($wrapper:expr, $($arg:tt)*) => {
        $wrapper.print_auto_no_indent(format!($($arg)*))
    }
}
pub(crate) use println_wrapped_no_indent;

impl TextWrapper {
    /// Print a string to STDOUT wrapped to the terminal width using the given subsequent indent
    /// width. The first line is not automatically indented so you can use bullets and other
    /// formatting characters.
    pub fn print(&mut self, subsequent_indent_width: usize, text: impl AsRef<str>) {
        let indent_string = self
            .indent_strings
            .entry(subsequent_indent_width)
            .or_insert_with(|| " ".repeat(subsequent_indent_width));
        let wrapping_options = self
            .wrapping_options
            .clone()
            .subsequent_indent(indent_string);
        println!("{}", textwrap::fill(text.as_ref(), wrapping_options));
    }

    /// The same as [`print()`][Self::print()], but it uses a heuristic to guess the subsequent
    /// indent width. This is the number of space and dash characters the input starts with, plus
    /// two.
    pub fn print_auto(&mut self, text: impl AsRef<str>) {
        let indent_width = Self::auto_indent_width(&text) + 2;

        self.print(indent_width, text)
    }

    /// The same as [`print_auto()`][Self::print_auto()], but doesn't indent subsequent lines.
    pub fn print_auto_no_indent(&mut self, text: impl AsRef<str>) {
        let indent_width = Self::auto_indent_width(&text);

        self.print(indent_width, text)
    }

    /// The number of characters until the start of the string, ignoring spaces and dashes.
    fn auto_indent_width(text: impl AsRef<str>) -> usize {
        text.as_ref()
            .chars()
            .take_while(|&c| c == ' ' || c == '-')
            .count()
    }
}
