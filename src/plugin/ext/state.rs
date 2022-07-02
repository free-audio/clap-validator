//! Abstractions for interacting with the `state` extension.

use anyhow::Result;
use clap_sys::ext::state::{clap_plugin_state, CLAP_EXT_STATE};
use clap_sys::stream::{clap_istream, clap_ostream};
use std::ffi::{c_void, CStr};
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use super::Extension;
use crate::plugin::instance::Plugin;
use crate::util::check_null_ptr;

/// Abstraction for the `state` extension covering the main thread functionality.
#[derive(Debug)]
pub struct State<'a> {
    plugin: &'a Plugin<'a>,
    state: NonNull<clap_plugin_state>,
}

/// An input stream backed by a slice.
#[repr(C)]
#[derive(Debug)]
struct InputStream<'a> {
    pub vtable: clap_istream,

    buffer: &'a [u8],
    /// The current position when reading from the buffer. This is needed because the plugin
    /// provides the buffer we should copy data into, and subsequent reads should continue from
    /// where we were left off.
    read_position: AtomicUsize,
}

/// An output stream backed by a vector.
#[repr(C)]
#[derive(Debug)]
struct OutputStream {
    pub vtable: clap_ostream,

    // In Rust-land this function is object is only used from a single thread and there's absolutely
    // no reason for the plugin to be calling the stream read and write methods from multiple
    // threads, but better be safe than sorry.
    buffer: Mutex<Vec<u8>>,
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

        if unsafe { (self.state.as_ref().save)(self.plugin.as_ptr(), &stream.vtable) } {
            Ok(stream.into_vec())
        } else {
            anyhow::bail!("'clap_plugin_state::save()' returned false.");
        }
    }

    /// Restore previously stored state. Returns an error if the plugin returned `false`.
    pub fn load(&self, state: &[u8]) -> Result<()> {
        let stream = InputStream::new(state);

        if unsafe { (self.state.as_ref().load)(self.plugin.as_ptr(), &stream.vtable) } {
            Ok(())
        } else {
            anyhow::bail!("'clap_plugin_state::load()' returned false.");
        }
    }
}

impl<'a> InputStream<'a> {
    /// Create a new input stream backed by a slice.
    pub fn new(buffer: &'a [u8]) -> Pin<Box<Self>> {
        Box::pin(InputStream {
            vtable: clap_istream {
                ctx: std::ptr::null_mut(),
                read: Self::read,
            },

            buffer,
            read_position: AtomicUsize::new(0),
        })
    }

    unsafe extern "C" fn read(stream: *const clap_istream, buffer: *mut c_void, size: u64) -> i64 {
        check_null_ptr!(0, stream, buffer);
        let this = &*(stream as *const Self);

        // TODO: Add a way to configure this stream to limit reads and writes to a specific size.
        //       We can use this to assert that the plugin can recover from this, because it's
        //       technically allowed.
        let current_pos = this.read_position.load(Ordering::Relaxed);
        let bytes_to_read = (this.buffer.len() - current_pos).min(size as usize);
        this.read_position
            .fetch_add(bytes_to_read, Ordering::Relaxed);

        std::slice::from_raw_parts_mut(buffer as *mut u8, bytes_to_read)
            .copy_from_slice(&this.buffer[current_pos..current_pos + bytes_to_read]);

        bytes_to_read as i64
    }
}

impl OutputStream {
    /// Create a new output stream backed by a vector.
    pub fn new() -> Pin<Box<Self>> {
        Box::pin(OutputStream {
            vtable: clap_ostream {
                ctx: std::ptr::null_mut(),
                write: Self::write,
            },

            buffer: Mutex::new(Vec::new()),
        })
    }

    /// Get the byte buffer from this stream.
    pub fn into_vec(self: Pin<Box<Self>>) -> Vec<u8> {
        // SAFETY: We can safely grab this inner buffer because this consumes the Box<Self>
        unsafe { Pin::into_inner_unchecked(self) }
            .buffer
            .into_inner()
            .unwrap()
    }

    unsafe extern "C" fn write(
        stream: *const clap_ostream,
        buffer: *const c_void,
        size: u64,
    ) -> i64 {
        check_null_ptr!(0, stream, buffer);
        let this = &*(stream as *const Self);

        // TODO: The same as in the InputStream
        this.buffer
            .lock()
            .unwrap()
            .extend_from_slice(std::slice::from_raw_parts(
                buffer as *const u8,
                size as usize,
            ));

        size as i64
    }
}
