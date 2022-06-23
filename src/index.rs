//! Utilities and data structures for indexing plugins.

use serde::Serialize;

use crate::plugin::ClapPlugin;

/// A list of known CLAP plugins found on this system. See [`index()`].
#[derive(Serialize)]
pub struct Index(Vec<ClapPlugin>);

/// Build an index of all CLAP plugins on this system. This finds all `.clap` files as specified in
/// [entry.h](https://github.com/free-audio/clap/blob/main/include/clap/entry.h), and lists all
/// plugins contained within those files. If a `.clap` file was found during the scan that could not
/// be read, then a warning will be printed.
pub fn index() -> Index {
    // TODO: Out of process scanning
    todo!()
}
