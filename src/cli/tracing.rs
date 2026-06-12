use std::path::Path;
use std::sync::{Mutex, OnceLock};

mod record;
mod writer;

pub use record::*;

static WRITER: OnceLock<Mutex<writer::TraceWriter>> = OnceLock::new();

pub fn install(path: &Path) {
    WRITER
        .set(Mutex::new(writer::TraceWriter::new(path)))
        .map_err(|_| ())
        .expect("instrumentation already started");
}

pub fn check_error() -> Result<(), String> {
    match WRITER.get() {
        Some(writer) => writer.lock().unwrap().check_error().map_err(|x| x.to_string()),
        None => Err("instrumentation not started".to_string()),
    }
}
