use super::{Recordable, Recorder};
use std::fs::File;
use std::io::{BufWriter, Error, Write};
use std::path::Path;
use std::time::Instant;

pub struct TraceWriter {
    file: Result<BufWriter<File>, Error>,
    start: Instant,
}

impl TraceWriter {
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        Self {
            file: File::create(path)
                .map(BufWriter::new)
                .and_then(|mut f| f.write_all(b"[\n").map(|_| f)),
            start: Instant::now(),
        }
    }

    pub fn check_error(&self) -> Result<(), &Error> {
        self.file.as_ref().map(|_| ())
    }

    pub fn write<A: Recordable>(&mut self, name: std::fmt::Arguments<'_>, cat: &str, tag: &str, args: &A) {
        if let Ok(file) = &mut self.file {
            let result = serde_json::to_writer(
                &mut *file,
                &TraceEvent {
                    name,
                    args: RecordableAsSerde(args),
                    ts: self.start.elapsed().as_micros(),
                    ph: tag,
                    cat,
                    id: 1,
                    pid: 1,
                },
            )
            .map_err(Error::other)
            .and_then(|_| file.write_all(b",\n"))
            .and_then(|_| file.flush());

            if let Err(e) = result {
                self.file = Err(e);
            }
        }
    }
}

/// An event that is written to the file
#[derive(serde::Serialize)]
struct TraceEvent<'a, N: serde::Serialize, A: serde::Serialize> {
    name: N,
    cat: &'a str,
    ts: u128,
    id: u64,
    pid: u64,
    ph: &'a str,
    args: A,
}

struct RecordableAsSerde<T: Recordable>(T);

impl<T: Recordable> serde::Serialize for RecordableAsSerde<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;

        struct SerdeRecorder<S: serde::ser::SerializeMap> {
            serializer: S,
            state: Result<(), S::Error>,
        }

        impl<S: serde::ser::SerializeMap> Recorder for SerdeRecorder<S> {
            fn record_value(&mut self, value: std::fmt::Arguments<'_>) {
                self.state = self.serializer.serialize_entry("", &value);
            }

            fn record_entry(&mut self, name: &str, record: &dyn Recordable) {
                if name.is_empty() {
                    record.record(self);
                } else {
                    self.state = self.serializer.serialize_entry(name, &RecordableAsSerde(record));
                }
            }
        }

        let mut recorder = SerdeRecorder {
            serializer: serializer.serialize_map(None)?,
            state: Ok(()),
        };

        self.0.record(&mut recorder);

        recorder.state?;
        recorder.serializer.end()
    }
}
