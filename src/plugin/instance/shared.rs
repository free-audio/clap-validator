use crate::cli::fail_test;
use crate::cli::tracing::{Span, record};
use crate::plugin::ext::Extension;
use crate::plugin::ext::audio_ports::AudioPorts;
use crate::plugin::ext::audio_ports_config::AudioPortsConfig;
use crate::plugin::ext::latency::Latency;
use crate::plugin::ext::note_ports::NotePorts;
use crate::plugin::ext::params::Params;
use crate::plugin::ext::preset_load::PresetLoad;
use crate::plugin::ext::state::State;
use crate::plugin::ext::tail::Tail;
use crate::plugin::ext::thread_pool::ThreadPool;
use crate::plugin::ext::voice_info::VoiceInfo;
use crate::plugin::instance::{CallbackEvent, Plugin, PluginStatus};
use crate::plugin::preset_discovery::LocationValue;
use crate::plugin::util::{self, CHECK_POINTER, Proxy, Proxyable, clap_call, cstr_ptr_to_string, validator_version};
use anyhow::{Context, Result};
use clap_sys::ext::audio_ports::*;
use clap_sys::ext::audio_ports_config::{CLAP_EXT_AUDIO_PORTS_CONFIG, clap_host_audio_ports_config};
use clap_sys::ext::latency::*;
use clap_sys::ext::log::*;
use clap_sys::ext::note_ports::*;
use clap_sys::ext::params::*;
use clap_sys::ext::preset_load::{CLAP_EXT_PRESET_LOAD, clap_host_preset_load};
use clap_sys::ext::state::{CLAP_EXT_STATE, clap_host_state};
use clap_sys::ext::tail::{CLAP_EXT_TAIL, clap_host_tail};
use clap_sys::ext::thread_check::{CLAP_EXT_THREAD_CHECK, clap_host_thread_check};
use clap_sys::ext::thread_pool::{CLAP_EXT_THREAD_POOL, clap_host_thread_pool};
use clap_sys::ext::voice_info::{CLAP_EXT_VOICE_INFO, clap_host_voice_info};
use clap_sys::factory::plugin_factory::clap_plugin_factory;
use clap_sys::factory::preset_discovery::clap_preset_discovery_location_kind;
use clap_sys::host::clap_host;
use clap_sys::id::clap_id;
use clap_sys::plugin::clap_plugin;
use clap_sys::version::CLAP_VERSION;
use crossbeam_utils::atomic::AtomicCell;
use std::ffi::{CStr, c_char, c_void};
use std::ptr::NonNull;
use std::sync::Mutex;
use std::sync::mpsc::{Sender, channel};
use std::thread::ThreadId;

#[derive(Debug, Clone, Copy)]
pub struct HostCapabilities {
    pub has_tail_extension: bool,
    pub has_latency_extension: bool,
    pub has_state_extension: bool,
    pub has_params_extension: bool,
    pub has_audio_ports_extension: bool,
    pub has_note_ports_extension: bool,
    pub has_thread_pool_extension: bool,

    pub supports_clap_dialect: bool,
    pub supports_midi_dialect: bool,
    pub can_rescan_audio_ports: bool,
}

impl Default for HostCapabilities {
    fn default() -> Self {
        Self {
            has_tail_extension: true,
            has_latency_extension: true,
            has_state_extension: true,
            has_params_extension: true,
            has_audio_ports_extension: true,
            has_note_ports_extension: true,
            has_thread_pool_extension: true,

            supports_clap_dialect: true,
            supports_midi_dialect: true,
            can_rescan_audio_ports: false,
        }
    }
}

/// Plugin instance state that is shared between the main thread, audio thread and any external unmanaged threads.
/// This struct also acts as the `clap_host` implementation for the plugin instance.
pub struct PluginShared {
    pub capabilities: HostCapabilities,

    pub callback_sender: Sender<CallbackEvent>,
    pub callback_error: Mutex<Option<anyhow::Error>>,

    /// The plugin's current state in terms of activation and processing status.
    status: AtomicCell<PluginStatus>,

    /// The plugin instance's main thread. Used for the main thread checks.
    pub main_thread_id: ThreadId,

    /// The plugin instance's audio thread, if it has one. Used for the audio thread checks.
    pub audio_thread_id: AtomicCell<Option<ThreadId>>,

    /// Whether the plugin has called `clap_host::request_callback()` and expects
    /// `clap_plugin::on_main_thread()` to be called on the main thread.
    pub requested_callback: AtomicCell<bool>,

    /// Whether the plugin has called `clap_host::request_restart()` and expects the plugin to be
    /// deactivated and subsequently reactivated.
    pub requested_restart: AtomicCell<bool>,

    /// Whether the plugin is currently being called from within a process call. This is used to
    /// check that certain functions (like thread_pool::request_exec()) are called from the process function.
    pub is_currently_in_process_call: AtomicCell<bool>,

    pub clap_plugin: *const clap_plugin,
}

unsafe impl Send for PluginShared {}
unsafe impl Sync for PluginShared {}

impl Proxyable for PluginShared {
    type Vtable = clap_host;

    fn init(&self) -> Self::Vtable {
        clap_host {
            clap_version: CLAP_VERSION,
            host_data: CHECK_POINTER,
            name: c"clap-validator".as_ptr(),
            vendor: c"Robbert van der Helm".as_ptr(),
            url: c"https://github.com/free-audio/clap-validator".as_ptr(),
            version: validator_version().as_ptr(),
            get_extension: Some(Self::clap_get_extension),
            request_restart: Some(Self::clap_request_restart),
            request_process: Some(Self::clap_request_process),
            request_callback: Some(Self::clap_request_callback),
        }
    }
}

impl PluginShared {
    /// Create a plugin instance and return the still uninitialized plugin. Returns an error if the
    /// plugin could not be created. The plugin instance will be registered with the host, and
    /// unregistered when this object is dropped again.
    ///
    /// # Safety
    /// The `factory` object must be valid.
    /// The caller must ensure that this is called from the OS main thread.
    pub unsafe fn create_plugin<'a>(
        factory: *const clap_plugin_factory,
        plugin_id: &CStr,
        capabilities: HostCapabilities,
    ) -> Result<Plugin<'a>> {
        let (callback_sender, callback_receiver) = channel();

        let shared = Proxy::new(PluginShared {
            capabilities,

            callback_sender,
            callback_error: Mutex::new(None),

            status: AtomicCell::new(PluginStatus::Uninitialized),
            main_thread_id: std::thread::current().id(),
            audio_thread_id: AtomicCell::new(None),
            requested_callback: AtomicCell::new(false),
            requested_restart: AtomicCell::new(false),
            is_currently_in_process_call: AtomicCell::new(false),

            clap_plugin: std::ptr::null(),
        });

        let span = Span::begin(
            "clap_plugin_factory::create_plugin",
            record!(
                plugin_id: plugin_id.to_string_lossy()
            ),
        );

        let clap_plugin = unsafe {
            clap_call! {
                factory=>create_plugin(factory, Proxy::vtable(&shared), plugin_id.as_ptr())
            }
        };

        span.finish(record!(result: format_args!("{:p}", clap_plugin)));

        if clap_plugin.is_null() {
            anyhow::bail!("'clap_plugin_factory::create_plugin({plugin_id:?})' returned a null pointer.");
        }

        unsafe {
            (&raw const shared.clap_plugin).cast_mut().write(clap_plugin);
        }

        Ok(Plugin {
            shared,
            callback_receiver,

            _library: std::marker::PhantomData,
            _thread: std::marker::PhantomData,
        })
    }

    /// Get the raw extension pointer for the extension `T`, if the plugin supports this extension.
    pub fn raw_extension<T: Extension>(&self) -> Option<NonNull<T::Struct>> {
        self.status().assert_is_not(PluginStatus::Uninitialized);

        for id in T::IDS {
            let span = Span::begin(
                "clap_plugin::get_extension",
                record! {
                    extension_id: id.to_string_lossy()
                },
            );

            let extension_ptr = unsafe {
                clap_call! { self.clap_plugin=>get_extension(self.clap_plugin, id.as_ptr()) }
            };

            span.finish(record!(result: format_args!("{:p}", extension_ptr)));

            if !extension_ptr.is_null() {
                return NonNull::new(extension_ptr as *mut T::Struct);
            }
        }

        None
    }

    /// Get a shared extension abstraction for the extension `T`, if the plugin supports this extension.
    pub fn get_extension<'a, T: Extension<Plugin = &'a Self>>(&'a self) -> Option<T> {
        unsafe { self.raw_extension::<T>().map(|ptr| T::new(self, ptr)) }
    }

    /// The plugin's current initialization status.
    pub fn status(&self) -> PluginStatus {
        self.status.load()
    }

    pub fn set_status(&self, status: PluginStatus) {
        self.status.store(status);
    }

    #[track_caller]
    fn wrap<R>(host: *const clap_host, function_name: &'static str, f: impl FnOnce(&Self) -> Result<R>) -> Option<R> {
        let state = unsafe {
            Proxy::<Self>::from_vtable(host).unwrap_or_else(|e| {
                fail_test!("{}: {}", function_name, e);
            })
        };

        if Proxy::vtable(&state).host_data != CHECK_POINTER {
            fail_test!("{}: plugin messed with the 'host_data' pointer", function_name);
        }

        match f(&state) {
            Ok(result) => Some(result),
            Err(error) => {
                log::error!("{:#}", error);

                let mut guard = state.callback_error.lock().unwrap();
                if guard.is_none() {
                    *guard = Some(error.context(function_name.to_string()));
                }

                None
            }
        }
    }

    /// Checks whether this is the main thread. If it is not, then an error indicating this can be
    /// retrieved using [`callback_error_check()`][Self::callback_error_check()]. Subsequent thread
    /// safety errors will not overwrite earlier ones.
    fn assert_main_thread(&self) -> Result<()> {
        let current_thread_id = std::thread::current().id();

        anyhow::ensure!(
            current_thread_id == self.main_thread_id,
            "The function may only be called from the main thread (thread {:?}), but it was called from thread {:?}.",
            self.main_thread_id,
            current_thread_id
        );

        Ok(())
    }

    /// Checks whether this is the audio thread. If it is not, then an error indicating this can be
    /// retrieved using [`callback_error_check()`][Self::callback_error_check()]. Subsequent thread
    /// safety errors will not overwrite earlier ones.
    fn assert_audio_thread(&self) -> Result<()> {
        let current_thread_id = std::thread::current().id();
        if self.audio_thread_id.load() != Some(current_thread_id) {
            if current_thread_id == self.main_thread_id {
                anyhow::bail!(
                    "This function may only be called from an audio thread, but it was called from the main thread."
                );
            } else {
                anyhow::bail!(
                    "This function may only be called from an audio thread, but it was called from an unknown thread \
                     ({:?}).",
                    current_thread_id
                );
            }
        }

        Ok(())
    }

    /// Checks whether this is **not** the audio thread. If it is, then an error indicating this can
    /// be retrieved using [`callback_error_check()`][Self::callback_error_check()]. Subsequent thread
    /// safety errors will not overwrite earlier ones.
    fn assert_not_audio_thread(&self) -> Result<()> {
        let current_thread_id = std::thread::current().id();
        if self.audio_thread_id.load() == Some(current_thread_id) {
            anyhow::bail!("This function was called from the audio thread, this is not allowed.");
        }
        Ok(())
    }

    /// Checks whether the plugin has the required extension(s). If it does not, then an error
    /// will be set. Subsequent errors will not overwrite earlier ones.
    fn assert_has_extension<T: Extension>(&self) -> Result<()> {
        anyhow::ensure!(
            self.status() != PluginStatus::Uninitialized,
            "Called while the plugin is uninitialized"
        );

        anyhow::ensure!(
            self.raw_extension::<T>().is_some(),
            "Plugin does not implement extension '{}'",
            T::IDS[0].to_string_lossy()
        );

        Ok(())
    }
}

// Extensions
impl PluginShared {
    const EXT_AUDIO_PORTS: clap_host_audio_ports = clap_host_audio_ports {
        is_rescan_flag_supported: Some(Self::ext_audio_ports_is_rescan_flag_supported),
        rescan: Some(Self::ext_audio_ports_rescan),
    };

    const EXT_NOTE_PORTS: clap_host_note_ports = clap_host_note_ports {
        supported_dialects: Some(Self::ext_note_ports_supported_dialects),
        rescan: Some(Self::ext_note_ports_rescan),
    };

    const EXT_PRESET_LOAD: clap_host_preset_load = clap_host_preset_load {
        on_error: Some(Self::ext_preset_load_on_error),
        loaded: Some(Self::ext_preset_load_loaded),
    };

    const EXT_PARAMS: clap_host_params = clap_host_params {
        rescan: Some(Self::ext_params_rescan),
        clear: Some(Self::ext_params_clear),
        request_flush: Some(Self::ext_params_request_flush),
    };

    const EXT_STATE: clap_host_state = clap_host_state {
        mark_dirty: Some(Self::ext_state_mark_dirty),
    };

    const EXT_THREAD_CHECK: clap_host_thread_check = clap_host_thread_check {
        is_audio_thread: Some(Self::ext_thread_check_is_audio_thread),
        is_main_thread: Some(Self::ext_thread_check_is_main_thread),
    };

    const EXT_LOG: clap_host_log = clap_host_log {
        log: Some(Self::ext_log_log),
    };

    const EXT_THREAD_POOL: clap_host_thread_pool = clap_host_thread_pool {
        request_exec: Some(Self::ext_thread_pool_request_exec),
    };

    const EXT_LATENCY: clap_host_latency = clap_host_latency {
        changed: Some(Self::ext_latency_changed),
    };

    const EXT_TAIL: clap_host_tail = clap_host_tail {
        changed: Some(Self::ext_tail_changed),
    };

    const EXT_VOICE_INFO: clap_host_voice_info = clap_host_voice_info {
        changed: Some(Self::ext_voice_info_changed),
    };

    const EXT_AUDIO_PORTS_CONFIG: clap_host_audio_ports_config = clap_host_audio_ports_config {
        rescan: Some(Self::ext_audio_ports_config_rescan),
    };

    unsafe extern "C" fn clap_get_extension(host: *const clap_host, extension_id: *const c_char) -> *const c_void {
        let extension_id_cstr = if extension_id.is_null() {
            None
        } else {
            Some(unsafe { CStr::from_ptr(extension_id) })
        };

        let span = Span::begin(
            "clap_host::get_extension",
            record! {
                extension_id: match extension_id_cstr {
                    Some(id) => id.to_string_lossy(),
                    None => "<null>".into()
                }
            },
        );

        // Right now there's no way to have the host only expose certain extensions. We can always
        // add that when test cases need it.
        Self::wrap(host, span.name(), |host| {
            let Some(extension_id_cstr) = extension_id_cstr else {
                anyhow::bail!("Null extension ID");
            };

            let extension_ptr =
                if extension_id_cstr == CLAP_EXT_AUDIO_PORTS && host.capabilities.has_audio_ports_extension {
                    &Self::EXT_AUDIO_PORTS as *const _ as *const c_void
                } else if extension_id_cstr == CLAP_EXT_NOTE_PORTS && host.capabilities.has_note_ports_extension {
                    &Self::EXT_NOTE_PORTS as *const _ as *const c_void
                } else if extension_id_cstr == CLAP_EXT_PRESET_LOAD {
                    &Self::EXT_PRESET_LOAD as *const _ as *const c_void
                } else if extension_id_cstr == CLAP_EXT_PARAMS && host.capabilities.has_params_extension {
                    &Self::EXT_PARAMS as *const _ as *const c_void
                } else if extension_id_cstr == CLAP_EXT_STATE && host.capabilities.has_state_extension {
                    &Self::EXT_STATE as *const _ as *const c_void
                } else if extension_id_cstr == CLAP_EXT_THREAD_CHECK {
                    &Self::EXT_THREAD_CHECK as *const _ as *const c_void
                } else if extension_id_cstr == CLAP_EXT_THREAD_POOL && host.capabilities.has_thread_pool_extension {
                    &Self::EXT_THREAD_POOL as *const _ as *const c_void
                } else if extension_id_cstr == CLAP_EXT_LOG {
                    &Self::EXT_LOG as *const _ as *const c_void
                } else if extension_id_cstr == CLAP_EXT_LATENCY && host.capabilities.has_latency_extension {
                    &Self::EXT_LATENCY as *const _ as *const c_void
                } else if extension_id_cstr == CLAP_EXT_TAIL && host.capabilities.has_tail_extension {
                    &Self::EXT_TAIL as *const _ as *const c_void
                } else if extension_id_cstr == CLAP_EXT_VOICE_INFO {
                    &Self::EXT_VOICE_INFO as *const _ as *const c_void
                } else if extension_id_cstr == CLAP_EXT_AUDIO_PORTS_CONFIG {
                    &Self::EXT_AUDIO_PORTS_CONFIG as *const _ as *const c_void
                } else {
                    std::ptr::null()
                };

            span.finish(record!(result: format_args!("{:p}", extension_ptr)));

            Ok(extension_ptr)
        })
        .unwrap_or_default()
    }

    unsafe extern "C" fn clap_request_restart(host: *const clap_host) {
        let span = Span::begin("clap_host::request_restart", ());

        Self::wrap(host, span.name(), |this| {
            this.requested_restart.store(true);
            Ok(())
        });
    }

    unsafe extern "C" fn clap_request_process(host: *const clap_host) {
        let span = Span::begin("clap_host::request_process", ());

        Self::wrap(host, span.name(), |this| {
            this.callback_sender.send(CallbackEvent::RequestProcess).unwrap();
            Ok(())
        });
    }

    unsafe extern "C" fn clap_request_callback(host: *const clap_host) {
        let span = Span::begin("clap_host::request_callback", ());

        Self::wrap(host, span.name(), |this| {
            this.requested_callback.store(true);
            Ok(())
        });
    }

    unsafe extern "C" fn ext_audio_ports_is_rescan_flag_supported(host: *const clap_host, flag: u32) -> bool {
        let span = Span::begin(
            "clap_host_audio_ports::is_rescan_flag_supported",
            record! {
                flag: match flag {
                    CLAP_AUDIO_PORTS_RESCAN_NAMES => "CLAP_AUDIO_PORTS_RESCAN_NAMES",
                    CLAP_AUDIO_PORTS_RESCAN_FLAGS => "CLAP_AUDIO_PORTS_RESCAN_FLAGS",
                    CLAP_AUDIO_PORTS_RESCAN_CHANNEL_COUNT => "CLAP_AUDIO_PORTS_RESCAN_CHANNEL_COUNT",
                    CLAP_AUDIO_PORTS_RESCAN_PORT_TYPE => "CLAP_AUDIO_PORTS_RESCAN_PORT_TYPE",
                    CLAP_AUDIO_PORTS_RESCAN_IN_PLACE_PAIR => "CLAP_AUDIO_PORTS_RESCAN_IN_PLACE_PAIR",
                    CLAP_AUDIO_PORTS_RESCAN_LIST => "CLAP_AUDIO_PORTS_RESCAN_LIST",
                    _ => "?"
                }
            },
        );

        Self::wrap(host, span.name(), |this| {
            this.assert_main_thread()?;
            this.assert_has_extension::<AudioPorts>()?;
            Ok(this.capabilities.can_rescan_audio_ports)
        })
        .unwrap_or(false)
    }

    unsafe extern "C" fn ext_audio_ports_rescan(host: *const clap_host, flags: u32) {
        let span = Span::begin(
            "clap_host_audio_ports::rescan",
            record! {
                rescan_names: flags & CLAP_AUDIO_PORTS_RESCAN_NAMES != 0,
                rescan_flags: flags & CLAP_AUDIO_PORTS_RESCAN_FLAGS != 0,
                rescan_channel_count: flags & CLAP_AUDIO_PORTS_RESCAN_CHANNEL_COUNT != 0,
                rescan_port_type: flags & CLAP_AUDIO_PORTS_RESCAN_PORT_TYPE != 0,
                rescan_in_place_pair: flags & CLAP_AUDIO_PORTS_RESCAN_IN_PLACE_PAIR != 0,
                rescan_list: flags & CLAP_AUDIO_PORTS_RESCAN_LIST != 0
            },
        );

        Self::wrap(host, span.name(), |this| {
            this.assert_main_thread()?;
            this.assert_has_extension::<AudioPorts>()?;

            anyhow::ensure!(
                this.capabilities.can_rescan_audio_ports,
                "Called when the host reported that it doesn't support rescanning audio ports."
            );

            if flags & CLAP_AUDIO_PORTS_RESCAN_NAMES != 0 {
                this.callback_sender.send(CallbackEvent::AudioPortsRescanNames).unwrap();
            }

            if (flags & CLAP_AUDIO_PORTS_RESCAN_FLAGS != 0)
                || (flags & CLAP_AUDIO_PORTS_RESCAN_CHANNEL_COUNT != 0)
                || (flags & CLAP_AUDIO_PORTS_RESCAN_PORT_TYPE != 0)
                || (flags & CLAP_AUDIO_PORTS_RESCAN_IN_PLACE_PAIR != 0)
            {
                anyhow::ensure!(
                    this.status() <= PluginStatus::Activated,
                    "Called while the plugin is active"
                );

                this.callback_sender.send(CallbackEvent::AudioPortsRescanInfo).unwrap();
            }

            if flags & CLAP_AUDIO_PORTS_RESCAN_LIST != 0 {
                anyhow::ensure!(
                    this.status() <= PluginStatus::Activated,
                    "Called while the plugin is active"
                );

                this.callback_sender.send(CallbackEvent::AudioPortsRescanList).unwrap();
            }

            Ok(())
        });
    }

    unsafe extern "C" fn ext_note_ports_supported_dialects(host: *const clap_host) -> clap_note_dialect {
        let span = Span::begin("clap_host_note_ports::supported_dialects", ());

        Self::wrap(host, span.name(), |this| {
            this.assert_main_thread()?;
            this.assert_has_extension::<NotePorts>()?;

            let mut flags = 0;

            if this.capabilities.supports_clap_dialect {
                flags |= CLAP_NOTE_DIALECT_CLAP;
            }

            if this.capabilities.supports_midi_dialect {
                flags |= CLAP_NOTE_DIALECT_MIDI | CLAP_NOTE_DIALECT_MIDI_MPE;
            }

            Ok(flags)
        })
        .unwrap_or(0)
    }

    unsafe extern "C" fn ext_note_ports_rescan(host: *const clap_host, flags: u32) {
        let span = Span::begin(
            "clap_host_note_ports::rescan",
            record! {
                rescan_names: flags & CLAP_NOTE_PORTS_RESCAN_NAMES != 0,
                rescan_all: flags & CLAP_NOTE_PORTS_RESCAN_ALL != 0
            },
        );

        Self::wrap(host, span.name(), |this| {
            this.assert_main_thread()?;
            this.assert_has_extension::<NotePorts>()?;

            if flags & CLAP_NOTE_PORTS_RESCAN_NAMES != 0 {
                this.callback_sender.send(CallbackEvent::NotePortsRescanNames).unwrap();
            }

            if flags & CLAP_NOTE_PORTS_RESCAN_ALL != 0 {
                anyhow::ensure!(
                    this.status() <= PluginStatus::Activated,
                    "Called while the plugin is active"
                );

                this.callback_sender.send(CallbackEvent::NotePortsRescanAll).unwrap();
            }

            Ok(())
        });
    }

    unsafe extern "C" fn ext_preset_load_on_error(
        host: *const clap_host,
        location_kind: clap_preset_discovery_location_kind,
        location: *const c_char,
        load_key: *const c_char,
        os_error: i32,
        msg: *const c_char,
    ) {
        Self::wrap(host, "clap_host_preset_load::on_error", |this| -> Result<()> {
            this.assert_main_thread()?;
            this.assert_has_extension::<PresetLoad>()?;

            let location = unsafe { LocationValue::new(location_kind, location) }
                .context("'clap_host_preset_load::on_error()' called with invalid location parameters")?;
            let load_key = unsafe { util::cstr_ptr_to_optional_string(load_key) }
                .context("'clap_host_preset_load::on_error()' called with an invalid load_key parameter")?;
            let msg = unsafe { util::cstr_ptr_to_mandatory_string(msg) }
                .context("'clap_host_preset_load::on_error()' called with an invalid msg parameter")?;

            if let Some(load_key) = &load_key {
                anyhow::bail!(
                    "Called for {location} with load key {load_key}, OS error code {os_error}, and the following \
                     error message: {msg}"
                );
            } else {
                anyhow::bail!(
                    "Called for {location} with no load key, OS error code {os_error}, and the following error \
                     message: {msg}"
                );
            }
        });
    }

    unsafe extern "C" fn ext_preset_load_loaded(
        host: *const clap_host,
        location_kind: clap_preset_discovery_location_kind,
        location: *const c_char,
        load_key: *const c_char,
    ) {
        Self::wrap(host, "clap_host_preset_load::loaded", |this| {
            this.assert_main_thread()?;
            this.assert_has_extension::<PresetLoad>()?;

            let _location = unsafe { LocationValue::new(location_kind, location) }
                .context("'Called with invalid location parameters")?;
            let _load_key = unsafe { util::cstr_ptr_to_optional_string(load_key) }
                .context("'Called with an invalid load_key parameter")?;

            log::debug!("TODO: Handle 'clap_host_preset_load::loaded()'");
            Ok(())
        });
    }

    unsafe extern "C" fn ext_params_rescan(host: *const clap_host, flags: clap_param_rescan_flags) {
        let span = Span::begin(
            "clap_host_params::rescan",
            record! {
                rescan_values: flags & CLAP_PARAM_RESCAN_VALUES != 0,
                rescan_text: flags & CLAP_PARAM_RESCAN_TEXT != 0,
                rescan_info: flags & CLAP_PARAM_RESCAN_INFO != 0,
                rescan_all: flags & CLAP_PARAM_RESCAN_ALL != 0
            },
        );

        Self::wrap(host, span.name(), |this| {
            this.assert_main_thread()?;
            this.assert_has_extension::<Params>()?;

            if flags & CLAP_PARAM_RESCAN_VALUES != 0 {
                this.callback_sender.send(CallbackEvent::ParamsRescanValues).unwrap();
            }

            if flags & CLAP_PARAM_RESCAN_TEXT != 0 {
                this.callback_sender.send(CallbackEvent::ParamsRescanText).unwrap();
            }

            if flags & CLAP_PARAM_RESCAN_INFO != 0 {
                this.callback_sender.send(CallbackEvent::ParamsRescanInfo).unwrap();
            }

            if flags & CLAP_PARAM_RESCAN_ALL != 0 {
                anyhow::ensure!(
                    this.status() <= PluginStatus::Activated,
                    "Called while the plugin is active"
                );

                this.callback_sender.send(CallbackEvent::ParamsRescanAll).unwrap();
            }

            Ok(())
        });
    }

    unsafe extern "C" fn ext_params_clear(host: *const clap_host, param_id: clap_id, flags: clap_param_clear_flags) {
        let span = Span::begin(
            "clap_host_params::clear",
            record! {
                param_id: param_id,
                clear_all: flags & CLAP_PARAM_CLEAR_ALL != 0,
                clear_modulations: flags & CLAP_PARAM_CLEAR_MODULATIONS != 0,
                clear_automations: flags & CLAP_PARAM_CLEAR_AUTOMATIONS != 0
            },
        );

        Self::wrap(host, span.name(), |this| {
            this.assert_main_thread()?;
            this.assert_has_extension::<Params>()?;
            log::debug!("TODO: Handle 'clap_host_params::clear()'");
            Ok(())
        });
    }

    unsafe extern "C" fn ext_params_request_flush(host: *const clap_host) {
        let span = Span::begin("clap_host_params::request_flush", ());

        Self::wrap(host, span.name(), |this| {
            this.assert_not_audio_thread()?;
            this.assert_has_extension::<Params>()?;
            this.callback_sender.send(CallbackEvent::RequestFlush).unwrap();
            Ok(())
        });
    }

    unsafe extern "C" fn ext_state_mark_dirty(host: *const clap_host) {
        let span = Span::begin("clap_host_state::mark_dirty", ());

        Self::wrap(host, span.name(), |this| {
            this.assert_main_thread()?;
            this.assert_has_extension::<State>()?;
            this.callback_sender.send(CallbackEvent::StateMarkDirty).unwrap();
            Ok(())
        });
    }

    // these 3 functions are explicitly uninstrumented to avoid overhead and unnecesary noise
    unsafe extern "C" fn ext_thread_check_is_main_thread(host: *const clap_host) -> bool {
        Self::wrap(host, "clap_host_thread_check::is_main_thread", |this| {
            Ok(this.main_thread_id == std::thread::current().id())
        })
        .unwrap_or(false)
    }

    unsafe extern "C" fn ext_thread_check_is_audio_thread(host: *const clap_host) -> bool {
        Self::wrap(host, "clap_host_thread_check::is_audio_thread", |this| {
            Ok(this.audio_thread_id.load() == Some(std::thread::current().id()))
        })
        .unwrap_or(false)
    }

    unsafe extern "C" fn ext_log_log(_host: *const clap_host, level: i32, msg: *const c_char) {
        let msg = match unsafe { cstr_ptr_to_string(msg) } {
            Ok(Some(msg)) => msg,
            Ok(None) => "<null>".into(),
            Err(_) => "<invalid utf-8>".into(),
        };

        match level {
            CLAP_LOG_ERROR => log::error!(target: "plugin::error", "{}", msg),
            CLAP_LOG_FATAL => log::error!(target: "plugin::fatal", "{}", msg),
            CLAP_LOG_WARNING => log::warn!(target: "plugin::warning", "{}", msg),
            CLAP_LOG_INFO => log::info!(target: "plugin::info", "{}", msg),
            CLAP_LOG_DEBUG => log::debug!(target: "plugin::debug", "{}", msg),
            CLAP_LOG_HOST_MISBEHAVING => log::error!(target: "plugin::host-misbehaving", "{}", msg),
            CLAP_LOG_PLUGIN_MISBEHAVING => log::error!(target: "plugin::plugin-misbehaving", "{}", msg),
            _ => log::debug!(target: "plugin", "{}", msg),
        }
    }

    unsafe extern "C" fn ext_latency_changed(host: *const clap_host) {
        let span = Span::begin("clap_host_latency::changed", ());

        Self::wrap(host, span.name(), |this| {
            this.assert_main_thread()?;
            this.assert_has_extension::<Latency>()?;

            anyhow::ensure!(
                this.status() == PluginStatus::Activating,
                "Must only be called within 'clap_plugin::activate'"
            );

            this.callback_sender.send(CallbackEvent::LatencyChanged).unwrap();

            Ok(())
        });
    }

    unsafe extern "C" fn ext_tail_changed(host: *const clap_host) {
        let span = Span::begin("clap_host_tail::changed", ());

        Self::wrap(host, span.name(), |this| {
            this.assert_audio_thread()?;
            this.assert_has_extension::<Tail>()?;
            this.callback_sender.send(CallbackEvent::TailChanged).unwrap();
            Ok(())
        });
    }

    unsafe extern "C" fn ext_voice_info_changed(host: *const clap_host) {
        let span = Span::begin("clap_host_voice_info::changed", ());

        Self::wrap(host, span.name(), |this| {
            this.assert_main_thread()?;
            this.assert_has_extension::<VoiceInfo>()?;
            this.callback_sender.send(CallbackEvent::VoiceInfoChanged).unwrap();
            Ok(())
        });
    }

    unsafe extern "C" fn ext_audio_ports_config_rescan(host: *const clap_host) {
        let span = Span::begin("clap_host_audio_ports_config::rescan", ());

        Self::wrap(host, span.name(), |this| {
            this.assert_main_thread()?;
            this.assert_has_extension::<AudioPortsConfig>()?;
            this.callback_sender
                .send(CallbackEvent::AudioPortsConfigRescan)
                .unwrap();
            Ok(())
        });
    }

    unsafe extern "C" fn ext_thread_pool_request_exec(host: *const clap_host, num_tasks: u32) -> bool {
        let span = Span::begin(
            "clap_host_thread_pool::request_exec",
            record! {
                num_tasks: num_tasks
            },
        );

        Self::wrap(host, span.name(), |this| {
            this.assert_audio_thread()?;
            this.assert_has_extension::<ThreadPool>()?;

            // Ensure this is called from within the process() function
            // We already checked that we're on the audio thread, so this is sufficient
            anyhow::ensure!(
                this.is_currently_in_process_call.load(),
                "Must only be called from within the 'clap_plugin::process' function."
            );

            let extension = this.get_extension::<ThreadPool>().unwrap();

            std::thread::scope(|s| {
                for i in 0..num_tasks {
                    s.spawn(move || {
                        extension.exec(i);
                    });
                }
            });

            Ok(true)
        })
        .unwrap_or(false)
    }
}
