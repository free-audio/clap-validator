//! Data structures and utilities for hosting plugins.

use anyhow::Result;
use clap_sys::ext::audio_ports::{clap_host_audio_ports, CLAP_EXT_AUDIO_PORTS};
use clap_sys::ext::note_ports::{
    clap_host_note_ports, clap_note_dialect, CLAP_EXT_NOTE_PORTS, CLAP_NOTE_DIALECT_CLAP,
    CLAP_NOTE_DIALECT_MIDI, CLAP_NOTE_DIALECT_MIDI_MPE,
};
use clap_sys::ext::params::{clap_host_params, clap_param_clear_flags, clap_param_rescan_flags};
use clap_sys::ext::state::{clap_host_state, CLAP_EXT_STATE};
use clap_sys::ext::thread_check::{clap_host_thread_check, CLAP_EXT_THREAD_CHECK};
use clap_sys::host::clap_host;
use clap_sys::id::clap_id;
use clap_sys::plugin::clap_plugin;
use clap_sys::version::CLAP_VERSION;
use crossbeam::atomic::AtomicCell;
use parking_lot::{Mutex, RwLock};
use std::collections::HashMap;
use std::ffi::{c_void, CStr};
use std::os::raw::c_char;
use std::pin::Pin;
use std::sync::Arc;
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

    /// These are the plugin instances taht were registered on this host. They're added here when
    /// the `Plugin` object is created, and they're removed when the object is dropped. This is used
    /// to keep track of audio threads and pending callbacks.
    pub instances: RwLock<HashMap<*const clap_plugin, PluginInstance>>,

    // These are the vtables for the extensions supported by the host
    clap_host_audio_ports: clap_host_audio_ports,
    clap_host_note_ports: clap_host_note_ports,
    clap_host_params: clap_host_params,
    clap_host_state: clap_host_state,
    clap_host_thread_check: clap_host_thread_check,
}

/// Runtime information about a plugin instance. This keeps track of pending callbacks and things
/// like audio threads.
#[derive(Debug, Default)]
pub struct PluginInstance {
    /// The plugin instance's audio thread, if it has one. Used for the audio thread checks.
    pub audio_thread: AtomicCell<Option<ThreadId>>,
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
                get_extension: Some(Self::get_extension),
                request_restart: Some(Self::request_restart),
                request_process: Some(Self::request_process),
                request_callback: Some(Self::request_callback),
            },
            main_thread_id: std::thread::current().id(),
            // If the plugin never makes callbacks from the wrong thread, then this will remain an
            // `None`. Otherwise this will be replaced by the first error.
            thread_safety_error: Mutex::new(None),

            instances: RwLock::new(HashMap::new()),

            clap_host_audio_ports: clap_host_audio_ports {
                is_rescan_flag_supported: Some(Self::ext_audio_ports_is_rescan_flag_supported),
                rescan: Some(Self::ext_audio_ports_rescan),
            },
            clap_host_note_ports: clap_host_note_ports {
                supported_dialects: Some(Self::ext_note_ports_supported_dialects),
                rescan: Some(Self::ext_note_ports_rescan),
            },
            clap_host_params: clap_host_params {
                rescan: Some(Self::ext_params_rescan),
                clear: Some(Self::ext_params_clear),
                request_flush: Some(Self::ext_params_request_flush),
            },
            clap_host_state: clap_host_state {
                mark_dirty: Some(Self::ext_state_mark_dirty),
            },
            clap_host_thread_check: clap_host_thread_check {
                is_main_thread: Some(Self::ext_thread_check_is_main_thread),
                is_audio_thread: Some(Self::ext_thread_check_is_audio_thread),
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
        match self.thread_safety_error.lock().take() {
            Some(err) => anyhow::bail!(err),
            None => Ok(()),
        }
    }

    /// Checks whether this is the main thread. If it is not, then an error indicating this can be
    /// retrieved using [`thread_safety_check()`][Self::thread_safety_check()]. Subsequent thread
    /// safety errors will not overwrite earlier ones.
    pub fn assert_main_thread(&self, function_name: &str) {
        let mut thread_safety_error = self.thread_safety_error.lock();
        let current_thread_id = std::thread::current().id();

        match *thread_safety_error {
            // Don't overwrite the first error
            None if std::thread::current().id() != self.main_thread_id => {
                *thread_safety_error = Some(format!(
                    "'{}' may only be called from the main thread (thread {:?}), but it was \
                     called from thread {:?}",
                    function_name, self.main_thread_id, current_thread_id
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
        let current_thread_id = std::thread::current().id();
        if !self.is_audio_thread(current_thread_id) {
            let mut thread_safety_error = self.thread_safety_error.lock();

            match *thread_safety_error {
                None if current_thread_id == self.main_thread_id => {
                    *thread_safety_error = Some(format!(
                        "'{}' may only be called from an audio thread, but it was called from the \
                         main thread",
                        function_name,
                    ))
                }
                None => {
                    *thread_safety_error = Some(format!(
                        "'{}' may only be called from an audio thread, but it was called from an \
                         unknown thread",
                        function_name,
                    ))
                }
                _ => (),
            }
        }
    }

    /// Checks whether this is **not** the audio thread. If it is, then an error indicating this can
    /// be retrieved using [`thread_safety_check()`][Self::thread_safety_check()]. Subsequent thread
    /// safety errors will not overwrite earlier ones.
    #[allow(unused)]
    pub fn assert_not_audio_thread(&self, function_name: &str) {
        let current_thread_id = std::thread::current().id();
        if self.is_audio_thread(current_thread_id) {
            let mut thread_safety_error = self.thread_safety_error.lock();
            if thread_safety_error.is_none() {
                *thread_safety_error = Some(format!(
                    "'{}' was called from an audio thread, this is not allowed",
                    function_name,
                ))
            }
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
        } else if extension_id_cstr == CLAP_EXT_STATE {
            &this.clap_host_state as *const _ as *const c_void
        } else if extension_id_cstr == CLAP_EXT_THREAD_CHECK {
            &this.clap_host_thread_check as *const _ as *const c_void
        } else {
            std::ptr::null()
        }
    }

    /// Returns whether the thread ID is one of the registered audio threads.
    fn is_audio_thread(&self, thread_id: ThreadId) -> bool {
        self.instances
            .read()
            .values()
            .any(|instance| instance.audio_thread.load() == Some(thread_id))
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

    unsafe extern "C" fn ext_params_rescan(
        host: *const clap_host,
        _flags: clap_param_rescan_flags,
    ) {
        check_null_ptr!((), host);
        let this = &*(host as *const Self);

        this.assert_main_thread("clap_host_params::rescan()");
        log::trace!("TODO: Add callbacks for 'clap_host_params::rescan()'");
    }

    unsafe extern "C" fn ext_params_clear(
        host: *const clap_host,
        _param_id: clap_id,
        _flags: clap_param_clear_flags,
    ) {
        check_null_ptr!((), host);
        let this = &*(host as *const Self);

        this.assert_main_thread("clap_host_params::clear()");
        log::trace!("TODO: Add callbacks for 'clap_host_params::clear()'");
    }

    unsafe extern "C" fn ext_params_request_flush(host: *const clap_host) {
        check_null_ptr!((), host);
        let this = &*(host as *const Self);

        this.assert_not_audio_thread("clap_host_params::request_flush()");
        log::trace!("TODO: Add callbacks for 'clap_host_params::request_flush()'");
    }

    unsafe extern "C" fn ext_state_mark_dirty(host: *const clap_host) {
        check_null_ptr!((), host);
        let this = &*(host as *const Self);

        this.assert_main_thread("clap_host_state::mark_dirty()");
        log::trace!("TODO: Add callbacks for 'clap_host_state::mark_dirty()'");
    }

    unsafe extern "C" fn ext_thread_check_is_main_thread(host: *const clap_host) -> bool {
        check_null_ptr!(false, host);
        let this = &*(host as *const Self);

        std::thread::current().id() == this.main_thread_id
    }

    unsafe extern "C" fn ext_thread_check_is_audio_thread(host: *const clap_host) -> bool {
        check_null_ptr!(false, host);
        let this = &*(host as *const Self);

        this.is_audio_thread(std::thread::current().id())
    }
}
