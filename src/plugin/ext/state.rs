//! Abstractions for interacting with the `state` extension.

use anyhow::Result;
use clap_sys::ext::state::{CLAP_EXT_STATE, clap_plugin_state};
use clap_sys::stream::{clap_istream, clap_ostream};
use parking_lot::Mutex;
use std::ffi::{CStr, c_void};
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::Extension;
use crate::plugin::instance::Plugin;
use crate::util::{check_null_ptr, unsafe_clap_call};

/// Abstraction for the `state` extension covering the main thread functionality.
#[derive(Debug)]
pub struct State<'a> {
    plugin: &'a Plugin<'a>,
    state: NonNull<clap_plugin_state>,
}

/// An input stream backed by a slice.
#[derive(Debug)]
struct InputStream<'a> {
    // The `ctx` pointer is set to this struct after creating the object
    vtable: clap_istream,

    buffer: &'a [u8],
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
    // The `ctx` pointer is set to this struct after creating the object
    vtable: clap_ostream,

    // In Rust-land this function is object is only used from a single thread and there's absolutely
    // no reason for the plugin to be calling the stream read and write methods from multiple
    // threads, but better be safe than sorry.
    buffer: Mutex<Vec<u8>>,
    /// The maximum number of bytes the plugin is allowed to write to this stream at a time, if the
    /// stream pretends to be buffered. This is used to test whether the plugin handles buffered
    /// streams correctly.
    max_write_size: Option<usize>,
}

impl<'a> Extension<&'a Plugin<'a>> for State<'a> {
    const EXTENSION_ID: &'static CStr = CLAP_EXT_STATE;

    type Struct = clap_plugin_state;

    fn new(plugin: &'a Plugin<'a>, extension_struct: NonNull<Self::Struct>) -> Self {
        Self {
            plugin,
            state: extension_struct,
        }
    }
}

impl State<'_> {
    /// Retrieve the plugin's state. Returns an error if the plugin returned `false`.
    pub fn save(&self) -> Result<Vec<u8>> {
        let stream = OutputStream::new();

        let state = self.state.as_ptr();
        let plugin = self.plugin.as_ptr();
        if unsafe_clap_call! { state=>save(plugin, &stream.vtable) } {
            Ok(stream.into_vec())
        } else {
            anyhow::bail!("'clap_plugin_state::save()' returned false.");
        }
    }

    /// Retrieve the plugin's state while limiting the number of bytes the plugin can write at a
    /// time. Returns an error if the plugin returned `false`.
    pub fn save_buffered(&self, max_bytes: usize) -> Result<Vec<u8>> {
        let stream = OutputStream::new().with_buffering(max_bytes);

        let state = self.state.as_ptr();
        let plugin = self.plugin.as_ptr();
        if unsafe_clap_call! { state=>save(plugin, stream.vtable()) } {
            Ok(stream.into_vec())
        } else {
            anyhow::bail!(
                "'clap_plugin_state::save()' returned false when only allowing the plugin to \
                 write {max_bytes} bytes at a time."
            );
        }
    }

    /// Restore previously stored state. Returns an error if the plugin returned `false`.
    pub fn load(&self, state: &[u8]) -> Result<()> {
        let stream = InputStream::new(state);

        let state = self.state.as_ptr();
        let plugin = self.plugin.as_ptr();
        if unsafe_clap_call! { state=>load(plugin, stream.vtable()) } {
            Ok(())
        } else {
            anyhow::bail!("'clap_plugin_state::load()' returned false.");
        }
    }

    /// Restore previously stored state while limiting the number of bytes the plugin can read at a
    /// time. Returns an error if the plugin returned `false`.
    pub fn load_buffered(&self, state: &[u8], max_bytes: usize) -> Result<()> {
        let stream = InputStream::new(state).with_buffering(max_bytes);

        let state = self.state.as_ptr();
        let plugin = self.plugin.as_ptr();
        if unsafe_clap_call! { state=>load(plugin, &stream.vtable) } {
            Ok(())
        } else {
            anyhow::bail!(
                "'clap_plugin_state::load()' returned false when only allowing the plugin to read \
                 {max_bytes} bytes at a time."
            );
        }
    }
}

impl<'a> InputStream<'a> {
    /// Create a new input stream backed by a slice.
    pub fn new(buffer: &'a [u8]) -> Pin<Box<Self>> {
        let mut stream = Box::pin(InputStream {
            vtable: clap_istream {
                // This is set to point to this object below
                ctx: std::ptr::null_mut(),
                read: Some(Self::read),
            },

            buffer,
            read_position: AtomicUsize::new(0),
            max_read_size: None,
        });

        stream.vtable.ctx = &*stream as *const Self as *mut c_void;

        stream
    }

    /// The stream's `clap_istream` vtable.
    pub fn vtable(self: &Pin<Box<Self>>) -> *const clap_istream {
        &self.vtable
    }

    /// Only allow `max_bytes` bytes to be read at a time. Useful for simulating buffered streams.
    pub fn with_buffering(mut self: Pin<Box<Self>>, max_bytes: usize) -> Pin<Box<Self>> {
        self.max_read_size = Some(max_bytes);
        self
    }

    unsafe extern "C" fn read(stream: *const clap_istream, buffer: *mut c_void, size: u64) -> i64 {
        unsafe {
            check_null_ptr!(stream, (*stream).ctx, buffer);
            let this = &*((*stream).ctx as *const Self);

            // The reads may be limited to a certain buffering size to test the plugin's capabilities
            let size = match this.max_read_size {
                Some(max_read_size) => size.min(max_read_size as u64),
                None => size,
            };

            let current_pos = this.read_position.load(Ordering::Relaxed);
            let bytes_to_read = (this.buffer.len() - current_pos).min(size as usize);
            this.read_position
                .fetch_add(bytes_to_read, Ordering::Relaxed);

            std::slice::from_raw_parts_mut(buffer as *mut u8, bytes_to_read)
                .copy_from_slice(&this.buffer[current_pos..current_pos + bytes_to_read]);

            bytes_to_read as i64
        }
    }
}

impl OutputStream {
    /// Create a new output stream backed by a vector.
    pub fn new() -> Pin<Box<Self>> {
        let mut stream = Box::pin(OutputStream {
            vtable: clap_ostream {
                // This is set to point to this object below
                ctx: std::ptr::null_mut(),
                write: Some(Self::write),
            },

            buffer: Mutex::new(Vec::new()),
            max_write_size: None,
        });

        stream.vtable.ctx = &*stream as *const Self as *mut c_void;

        stream
    }

    /// The stream's `clap_ostream` vtable.
    pub fn vtable(self: &Pin<Box<Self>>) -> *const clap_ostream {
        &self.vtable
    }

    /// Only allow `max_bytes` bytes to be written at a time. Useful for simulating buffered
    /// streams.
    pub fn with_buffering(mut self: Pin<Box<Self>>, max_bytes: usize) -> Pin<Box<Self>> {
        self.max_write_size = Some(max_bytes);
        self
    }

    /// Get the byte buffer from this stream.
    pub fn into_vec(self: Pin<Box<Self>>) -> Vec<u8> {
        // SAFETY: We can safely grab this inner buffer because this consumes the Box<Self>
        unsafe { Pin::into_inner_unchecked(self) }
            .buffer
            .into_inner()
    }

    unsafe extern "C" fn write(
        stream: *const clap_ostream,
        buffer: *const c_void,
        size: u64,
    ) -> i64 {
        unsafe {
            check_null_ptr!(stream, (*stream).ctx, buffer);
            let this = &*((*stream).ctx as *const Self);

            // The writes may be limited to a certain buffering size to test the plugin's capabilities
            let size = match this.max_write_size {
                Some(max_write_size) => size.min(max_write_size as u64),
                None => size,
            };

            this.buffer
                .lock()
                .extend_from_slice(std::slice::from_raw_parts(
                    buffer as *const u8,
                    size as usize,
                ));

            size as i64
        }
    }
}
