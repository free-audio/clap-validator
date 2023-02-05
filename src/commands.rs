//! All the different commands for the cli. Split up into modules and functions to make it a bit
//! easier to navigate.

use std::borrow::Borrow;
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

impl TextWrapper {
    /// Print a string to STDOUT wrapped to the terminal width using the given subsequent indent
    /// width. The first line is not automatically indented so you can use bullets and other
    /// formatting characters.
    pub fn print(&mut self, subsequent_indent_width: usize, text: impl Borrow<str>) {
        let indent_string = self
            .indent_strings
            .entry(subsequent_indent_width)
            .or_insert_with(|| " ".repeat(subsequent_indent_width));
        let wrapping_options = self
            .wrapping_options
            .clone()
            .subsequent_indent(&indent_string);
        println!("{}", textwrap::fill(text.borrow(), wrapping_options));
    }
}
