//! Abstractions for interacting with the `state` extension.

use super::Extension;
use crate::cli::fail_test;
use crate::cli::tracing::{Span, record};
use crate::plugin::instance::Plugin;
use crate::plugin::util::{CHECK_POINTER, Proxy, Proxyable, clap_call};
use anyhow::Result;
use clap_sys::ext::state::{CLAP_EXT_STATE, clap_plugin_state};
use clap_sys::stream::{clap_istream, clap_ostream};
use std::ffi::{CStr, c_void};
use std::ptr::NonNull;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::ThreadId;

/// Abstraction for the `state` extension covering the main thread functionality.
pub struct State<'a> {
    plugin: &'a Plugin<'a>,
    state: NonNull<clap_plugin_state>,
}

/// An input stream backed by a slice.
#[derive(Debug)]
struct InputStream<'a> {
    /// The thread ID that created this stream. Used to verify that the plugin is calling the stream
    /// methods from the same thread.
    expected_thread_id: ThreadId,

    /// The buffer to read from.
    read_buffer: &'a [u8],

    /// The current position when reading from the buffer. This is needed because the plugin
    /// provides the buffer we should copy data into, and subsequent reads should continue from
    /// where we were left off.
    read_position: AtomicUsize,
    /// The maximum number of bytes this stream will return at a time, if the stream pretends to be
    /// buffered. This is used to test whether the plugin handles buffered streams correctly.
    max_read_size: Option<usize>,
}

/// An output stream backed by a vector.
#[derive(Debug)]
struct OutputStream {
    /// The thread ID that created this stream. Used to verify that the plugin is calling the stream
    /// methods from the same thread.
    expected_thread_id: ThreadId,

    // In Rust-land this function is object is only used from a single thread and there's absolutely
    // no reason for the plugin to be calling the stream read and write methods from multiple
    // threads, but better be safe than sorry.
    write_buffer: Mutex<Vec<u8>>,

    /// The maximum number of bytes the plugin is allowed to write to this stream at a time, if the
    /// stream pretends to be buffered. This is used to test whether the plugin handles buffered
    /// streams correctly.
    max_write_size: Option<usize>,
}

impl<'a> Extension for State<'a> {
    const IDS: &'static [&'static CStr] = &[CLAP_EXT_STATE];

    type Plugin = &'a Plugin<'a>;
    type Struct = clap_plugin_state;

    unsafe fn new(plugin: &'a Plugin<'a>, state: NonNull<Self::Struct>) -> Self {
        Self { plugin, state }
    }
}

impl State<'_> {
    /// Retrieve the plugin's state. Returns an error if the plugin returned `false`.
    pub fn save(&self) -> Result<Vec<u8>> {
        let stream = OutputStream::new(None);
        let state = self.state.as_ptr();
        let plugin = self.plugin.as_ptr();

        let span = Span::begin("clap_plugin_state::save", ());
        let result = unsafe {
            clap_call! { state=>save(plugin, Proxy::vtable(&stream)) }
        };

        span.finish(record!(result: result));

        if result {
            Ok(stream.take())
        } else {
            anyhow::bail!("'clap_plugin_state::save()' returned false");
        }
    }

    /// Retrieve the plugin's state while limiting the number of bytes the plugin can write at a
    /// time. Returns an error if the plugin returned `false`.
    pub fn save_buffered(&self, max_bytes: usize) -> Result<Vec<u8>> {
        let stream = OutputStream::new(Some(max_bytes));
        let state = self.state.as_ptr();
        let plugin = self.plugin.as_ptr();

        let span = Span::begin("clap_plugin_state::save", record! { max_bytes: max_bytes });
        let result = unsafe {
            clap_call! { state=>save(plugin, Proxy::vtable(&stream)) }
        };

        span.finish(record!(result: result));

        if result {
            Ok(stream.take())
        } else {
            anyhow::bail!(
                "'clap_plugin_state::save()' returned false when only allowing the plugin to write {max_bytes} bytes \
                 at a time"
            );
        }
    }

    /// Restore previously stored state. Returns an error if the plugin returned `false`.
    pub fn load(&self, state: &[u8]) -> Result<()> {
        let stream = InputStream::new(state, None);
        let state = self.state.as_ptr();
        let plugin = self.plugin.as_ptr();

        let span = Span::begin("clap_plugin_state::load", ());
        let result = unsafe {
            clap_call! { state=>load(plugin, Proxy::vtable(&stream)) }
        };

        span.finish(record!(result: result));

        if result {
            Ok(())
        } else {
            anyhow::bail!("'clap_plugin_state::load()' returned false");
        }
    }

    /// Restore previously stored state while limiting the number of bytes the plugin can read at a
    /// time. Returns an error if the plugin returned `false`.
    pub fn load_buffered(&self, state: &[u8], max_bytes: usize) -> Result<()> {
        let stream = InputStream::new(state, Some(max_bytes));

        let state = self.state.as_ptr();
        let plugin = self.plugin.as_ptr();

        let span = Span::begin("clap_plugin_state::load", record! { max_bytes: max_bytes });
        let result = unsafe {
            clap_call! { state=>load(plugin, Proxy::vtable(&stream)) }
        };

        span.finish(record!(result: result));

        if result {
            Ok(())
        } else {
            anyhow::bail!(
                "'clap_plugin_state::load()' returned false when only allowing the plugin to read {max_bytes} bytes \
                 at a time"
            );
        }
    }
}

impl<'a> Proxyable for InputStream<'a> {
    type Vtable = clap_istream;

    fn init(&self) -> Self::Vtable {
        clap_istream {
            ctx: CHECK_POINTER,
            read: Some(Self::read),
        }
    }
}

impl Proxyable for OutputStream {
    type Vtable = clap_ostream;

    fn init(&self) -> Self::Vtable {
        clap_ostream {
            ctx: CHECK_POINTER,
            write: Some(Self::write),
        }
    }
}

impl<'a> InputStream<'a> {
    /// Create a new input stream backed by a slice.
    pub fn new(buffer: &'a [u8], max_read_size: Option<usize>) -> Proxy<Self> {
        Proxy::new(InputStream {
            read_buffer: buffer,
            expected_thread_id: std::thread::current().id(),
            read_position: AtomicUsize::new(0),
            max_read_size,
        })
    }

    unsafe extern "C" fn read(stream: *const clap_istream, buffer: *mut c_void, size: u64) -> i64 {
        let span = Span::begin(
            "clap_istream::read",
            record! { buffer: format_args!("{:p}", buffer), size: size },
        );

        unsafe {
            let state = Proxy::<Self>::from_vtable(stream).unwrap_or_else(|e| {
                fail_test!("clap_istream::read: {}", e);
            });

            if Proxy::vtable(&state).ctx != CHECK_POINTER {
                fail_test!("clap_istream::read: plugin messed with the 'ctx' pointer");
            }

            if state.expected_thread_id != std::thread::current().id() {
                fail_test!("clap_istream::read: called from a different thread than the one that created the stream");
            }

            // The reads may be limited to a certain buffering size to test the plugin's capabilities
            let size = match state.max_read_size {
                Some(max_read_size) => size.min(max_read_size as u64),
                None => size,
            };

            let current_pos = state.read_position.load(Ordering::Relaxed);
            let bytes_to_read = (state.read_buffer.len() - current_pos).min(size as usize);
            state.read_position.fetch_add(bytes_to_read, Ordering::Relaxed);

            std::slice::from_raw_parts_mut(buffer as *mut u8, bytes_to_read)
                .copy_from_slice(&state.read_buffer[current_pos..current_pos + bytes_to_read]);

            span.finish(record! { bytes_read: bytes_to_read });
            bytes_to_read as i64
        }
    }
}

impl OutputStream {
    /// Create a new output stream backed by a vector.
    pub fn new(max_write_size: Option<usize>) -> Proxy<Self> {
        Proxy::new(OutputStream {
            expected_thread_id: std::thread::current().id(),
            write_buffer: Mutex::new(Vec::new()),
            max_write_size,
        })
    }

    /// Take the contents of the write buffer.
    pub fn take(&self) -> Vec<u8> {
        std::mem::take(&mut *self.write_buffer.lock().unwrap())
    }

    unsafe extern "C" fn write(stream: *const clap_ostream, buffer: *const c_void, size: u64) -> i64 {
        let span = Span::begin(
            "clap_ostream::write",
            record! { buffer: format_args!("{:p}", buffer), size: size },
        );

        unsafe {
            let state = Proxy::<Self>::from_vtable(stream).unwrap_or_else(|e| {
                fail_test!("clap_ostream::write: {}", e);
            });

            if Proxy::vtable(&state).ctx != CHECK_POINTER {
                fail_test!("clap_ostream::write: plugin messed with the 'ctx' pointer");
            }

            if buffer.is_null() {
                fail_test!("clap_ostream::write: 'buffer' pointer is null");
            }

            if state.expected_thread_id != std::thread::current().id() {
                fail_test!("clap_ostream::write: called from a different thread than the one that created the stream");
            }

            // The writes may be limited to a certain buffering size to test the plugin's capabilities
            let size = match state.max_write_size {
                Some(max_write_size) => size.min(max_write_size as u64),
                None => size,
            };

            state
                .write_buffer
                .lock()
                .unwrap()
                .extend_from_slice(std::slice::from_raw_parts(buffer as *const u8, size as usize));

            span.finish(record! { bytes_written: size });
            size as i64
        }
    }
}
