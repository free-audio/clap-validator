//! Data structures and utilities for hosting plugins.

use clap_sys::host::clap_host;
use clap_sys::version::CLAP_VERSION;
use std::ffi::c_void;
use std::os::raw::c_char;
use std::pin::Pin;
use std::sync::Arc;

#[derive(Debug)]
#[repr(C)]
pub struct ClapHost {
    /// The function vtable for this CLAP host instance. This is kept in this struct so we can
    /// easily back a `clap_host` pointer to an object instance.
    clap_host: clap_host,
}

impl ClapHost {
    /// Initialize a CLAP host. The `Pin` is necessary to prevent moving the object out of the
    /// `Box`, since that would break pointers to  the `ClapHost`.
    pub fn new() -> Pin<Arc<ClapHost>> {
        Pin::new(Arc::new(ClapHost {
            clap_host: clap_host {
                clap_version: CLAP_VERSION,
                host_data: std::ptr::null_mut(),
                name: b"clapval\0".as_ptr() as *const c_char,
                vendor: b"Robbert van der Helm\0".as_ptr() as *const c_char,
                url: b"https://github.com/robbert-vdh/clapval\0".as_ptr() as *const c_char,
                version: b"0.1.0\0".as_ptr() as *const c_char,
                get_extension: Self::get_extension,
                request_restart: Self::request_restart,
                request_process: Self::request_process,
                request_callback: Self::request_callback,
            },
        }))
    }

    /// Get the pointer to this host's vtable.
    pub fn as_ptr(self: &Pin<Arc<ClapHost>>) -> *const clap_host {
        // The value will not move, since this `ClapHost` can only be constructed as a
        // `Pin<Arc<ClapHost>>`
        &self.clap_host
    }

    unsafe extern "C" fn get_extension(
        _host: *const clap_host,
        _extension_id: *const c_char,
    ) -> *const c_void {
        eprintln!("TODO: Add hooks for clap_host::get_extension()");

        std::ptr::null()
    }

    unsafe extern "C" fn request_restart(_host: *const clap_host) {
        // TODO: These functions should do thread checks. If any of these functions are called from
        //       an incorrect thread, then there should be some way to retrieve a `Result<()>`
        //       containing an `Err` with an explanation of what went wrong.

        eprintln!("TODO: Add hooks for clap_host::request_restart()");
    }

    unsafe extern "C" fn request_process(_host: *const clap_host) {
        eprintln!("TODO: Add hooks for clap_host::request_process()");
    }

    unsafe extern "C" fn request_callback(_host: *const clap_host) {
        eprintln!("TODO: Add hooks for clap_host::request_callback()");
    }
}
