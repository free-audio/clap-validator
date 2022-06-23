//! Utilities and data structures for indexing plugins.

use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::plugin::ClapMetadata;

/// A containing metadata for all CLAP plugins found on this system. Each plugin path in the map
/// contains zero or more plugins. See [`index()`].
#[derive(Debug, Serialize)]
pub struct Index(pub HashMap<PathBuf, ClapMetadata>);

/// Build an index of all CLAP plugins on this system. This finds all `.clap` files as specified in
/// [entry.h](https://github.com/free-audio/clap/blob/main/include/clap/entry.h), and lists all
/// plugins contained within those files. If a `.clap` file was found during the scan that could not
/// be read, then a warning will be printed.
pub fn index() -> Index {
    // TODO: Out of process scanning
    todo!()
}
