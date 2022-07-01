//! Data structures and utilities for hosting plugins.

use anyhow::Result;
use clap_sys::ext::audio_ports::{clap_host_audio_ports, CLAP_EXT_AUDIO_PORTS};
use clap_sys::ext::note_ports::{
    clap_host_note_ports, clap_note_dialect, CLAP_EXT_NOTE_PORTS, CLAP_NOTE_DIALECT_CLAP,
    CLAP_NOTE_DIALECT_MIDI, CLAP_NOTE_DIALECT_MIDI_MPE,
};
use clap_sys::host::clap_host;
use clap_sys::version::CLAP_VERSION;
use std::ffi::{c_void, CStr};
use std::os::raw::c_char;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;

use crate::util::check_null_ptr;

/// An abstraction for a CLAP plugin host. Its behavior can be configured through callbacks, and it
/// checks whether the calling thread matches up when any of its functions are called by the plugin.
/// A `Result` indicating the first failure, of any, can be retrieved by calling the
/// [`thread_safety_check()`][Self::thread_safety_check()] method.
///
/// TODO: Add a `ClapHostConfig` to set callbacks. Right now the host-versions of all plugin
///       extensions used by the validator are exposed, and other than thread checks none of the
///       callbacks do anything right now.
#[derive(Debug)]
#[repr(C)]
pub struct ClapHost {
    /// The function vtable for this CLAP host instance. This is kept in this struct so we can
    /// easily cast a `clap_host` pointer to an object instance.
    clap_host: clap_host,
    /// The ID of the main thread.
    main_thread_id: ThreadId,
    /// A description of the first thread safety error encountered by this `ClapHost`, if any. This
    /// is used to check that the plugin called any host callbacks from the correct thread after the
    /// test has succeeded.
    thread_safety_error: Mutex<Option<String>>,

    // These are the vtables for the extensions supported by the host
    clap_host_audio_ports: clap_host_audio_ports,
    clap_host_note_ports: clap_host_note_ports,
}

impl ClapHost {
    /// Initialize a CLAP host. The thread this object is created on will be designated as the main
    /// thread for the purposes of the thread safety checks. The `Pin` is necessary to prevent
    /// moving the object out of the `Box`, since that would break pointers to the `ClapHost`.
    pub fn new() -> Pin<Arc<ClapHost>> {
        Pin::new(Arc::new(ClapHost {
            clap_host: clap_host {
                clap_version: CLAP_VERSION,
                host_data: std::ptr::null_mut(),
                name: b"clap-validator\0".as_ptr() as *const c_char,
                vendor: b"Robbert van der Helm\0".as_ptr() as *const c_char,
                url: b"https://github.com/robbert-vdh/clap-validator\0".as_ptr() as *const c_char,
                version: b"0.1.0\0".as_ptr() as *const c_char,
                get_extension: Self::get_extension,
                request_restart: Self::request_restart,
                request_process: Self::request_process,
                request_callback: Self::request_callback,
            },
            main_thread_id: std::thread::current().id(),
            // If the plugin never makes callbacks from the wrong thread, then this will remain an
            // `None`. Otherwise this will be replaced by the first error.
            thread_safety_error: Mutex::new(None),

            clap_host_audio_ports: clap_host_audio_ports {
                is_rescan_flag_supported: Self::ext_audio_ports_is_rescan_flag_supported,
                rescan: Self::ext_audio_ports_rescan,
            },
            clap_host_note_ports: clap_host_note_ports {
                supported_dialects: Self::ext_note_ports_supported_dialects,
                rescan: Self::ext_note_ports_rescan,
            },
        }))
    }

    /// Get the pointer to this host's vtable.
    pub fn as_ptr(self: &Pin<Arc<ClapHost>>) -> *const clap_host {
        // The value will not move, since this `ClapHost` can only be constructed as a
        // `Pin<Arc<ClapHost>>`
        &self.clap_host
    }

    pub fn thread_safety_check(&self) -> Result<()> {
        #[allow(clippy::significant_drop_in_scrutinee)]
        match self.thread_safety_error.lock().unwrap().take() {
            Some(err) => anyhow::bail!(err),
            None => Ok(()),
        }
    }

    /// Checks whether this is the main thread. If it is not, then an error indicating this can be
    /// retrieved using [`thread_safety_check()`][Self::thread_safety_check()]. Subsequent thread
    /// safety errors will not overwrite earlier ones.
    //
    // TODO: Remove these unused attributes once we implement extensions
    pub fn assert_main_thread(&self, function_name: &str) {
        let mut thread_safety_error = self.thread_safety_error.lock().unwrap();
        let current_thread_id = std::thread::current().id();

        #[allow(clippy::significant_drop_in_scrutinee)]
        match *thread_safety_error {
            // Don't overwrite the first error
            None if std::thread::current().id() != self.main_thread_id => {
                *thread_safety_error = Some(format!(
                    "'{}' may only be called from the main thread (thread {:?}), but it was called from thread {:?}",
                    function_name,
                    self.main_thread_id,
                    current_thread_id
                ))
            }
            _ => (),
        }
    }

    /// Checks whether this is the audio thread. If it is not, then an error indicating this can be
    /// retrieved using [`thread_safety_check()`][Self::thread_safety_check()]. Subsequent thread
    /// safety errors will not overwrite earlier ones.
    #[allow(unused)]
    pub fn assert_audio_thread(&self, function_name: &str) {
        let mut thread_safety_error = self.thread_safety_error.lock().unwrap();
        let current_thread_id = std::thread::current().id();

        #[allow(clippy::significant_drop_in_scrutinee)]
        match *thread_safety_error {
            // Don't overwrite the first error
            // TODO: This doesn't necessarily check for 'the' audio thread, although in practice that shouldn't matter
            None if std::thread::current().id() == self.main_thread_id => {
                *thread_safety_error = Some(format!(
                    "'{}' may only be called from an audio thread, but it was called from the main thread",
                    function_name,
                ))
            }
            _ => (),
        }
    }

    unsafe extern "C" fn get_extension(
        host: *const clap_host,
        extension_id: *const c_char,
    ) -> *const c_void {
        check_null_ptr!(std::ptr::null(), host, extension_id);
        let this = &*(host as *const Self);

        // Right now there's no way to have the host only expose certain extensions. We can always
        // add that when test cases need it.
        let extension_id_cstr = CStr::from_ptr(extension_id);
        if extension_id_cstr == CLAP_EXT_AUDIO_PORTS {
            &this.clap_host_audio_ports as *const _ as *const c_void
        } else if extension_id_cstr == CLAP_EXT_NOTE_PORTS {
            &this.clap_host_note_ports as *const _ as *const c_void
        } else {
            std::ptr::null()
        }
    }

    unsafe extern "C" fn request_restart(host: *const clap_host) {
        check_null_ptr!((), host);

        log::trace!("TODO: Add callbacks for 'clap_host::request_restart()'");
    }

    unsafe extern "C" fn request_process(host: *const clap_host) {
        check_null_ptr!((), host);

        log::trace!("TODO: Add callbacks for 'clap_host::request_process()'");
    }

    unsafe extern "C" fn request_callback(host: *const clap_host) {
        check_null_ptr!((), host);

        log::trace!("TODO: Add callbacks for 'clap_host::request_callback()'");
    }

    unsafe extern "C" fn ext_audio_ports_is_rescan_flag_supported(
        host: *const clap_host,
        _flag: u32,
    ) -> bool {
        check_null_ptr!(false, host);
        let this = &*(host as *const Self);

        this.assert_main_thread("clap_host_audio_ports::is_rescan_flag_supported()");
        log::trace!("TODO: Add callbacks for 'clap_host_audio_ports::is_rescan_flag_supported()'");

        true
    }

    unsafe extern "C" fn ext_audio_ports_rescan(host: *const clap_host, _flags: u32) {
        check_null_ptr!((), host);
        let this = &*(host as *const Self);

        this.assert_main_thread("clap_host_audio_ports::rescan()");
        log::trace!("TODO: Add callbacks for 'clap_host_audio_ports::rescan()'");
    }

    unsafe extern "C" fn ext_note_ports_supported_dialects(
        host: *const clap_host,
    ) -> clap_note_dialect {
        check_null_ptr!(0, host);
        let this = &*(host as *const Self);

        this.assert_main_thread("clap_host_note_ports::supported_dialects()");

        CLAP_NOTE_DIALECT_CLAP | CLAP_NOTE_DIALECT_MIDI | CLAP_NOTE_DIALECT_MIDI_MPE
    }

    unsafe extern "C" fn ext_note_ports_rescan(host: *const clap_host, _flags: u32) {
        check_null_ptr!((), host);
        let this = &*(host as *const Self);

        this.assert_main_thread("clap_host_note_ports::rescan()");
        log::trace!("TODO: Add callbacks for 'clap_host_note_ports::rescan()'");
    }
}
