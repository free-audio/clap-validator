//! All the different commands for the cli. Split up into modules and functions to make it a bit
//! easier to navigate.

use std::borrow::Borrow;

pub mod list;
pub mod validate;

/// A helper for printing terminal wrapped and indentend strings to STDOUT.
pub struct TextWrapper {
    // This borrows the indent string once added, so we'll store the indent string separately on
    // this struct and only add it to the options when we do the formatting
    wrapping_options: textwrap::Options<'static>,
    indent_string: String,
}

impl TextWrapper {
    /// Create a text wrapper that indents subsequent lines with `subsequent_indent` spaces.
    pub fn new(subsequent_indent: usize) -> Self {
        Self {
            wrapping_options: textwrap::Options::with_termwidth(),
            indent_string: " ".repeat(subsequent_indent),
        }
    }

    /// Print a string to STDOUT wrapped to the terminal width using this struct's subsequent indent
    /// width. The first line is not automatically indented so you can use bullets and other
    /// formatting characters.
    pub fn print(&self, text: impl Borrow<str>) {
        let wrapping_options = self
            .wrapping_options
            .clone()
            .subsequent_indent(&self.indent_string);
        println!("{}", textwrap::fill(text.borrow(), wrapping_options));
    }
}
