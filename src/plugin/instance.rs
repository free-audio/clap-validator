//! Abstractions for single CLAP plugin instances for main thread interactions.

use anyhow::Result;
use clap_sys::plugin::clap_plugin;
use clap_sys::plugin_factory::clap_plugin_factory;
use std::ffi::CStr;
use std::marker::PhantomData;
use std::ops::Deref;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::Arc;

use super::audio_thread::PluginAudioThread;
use super::ext::Extension;
use super::library::PluginLibrary;
use crate::host::{ClapHost, HostPluginInstance};
use crate::util::unsafe_clap_call;

/// A `Send+Sync` wrapper around `*const clap_plugin`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct PluginHandle(pub NonNull<clap_plugin>);

unsafe impl Send for PluginHandle {}
unsafe impl Sync for PluginHandle {}

/// A CLAP plugin instance. The plugin will be deinitialized when this object is dropped. All
/// functions here are callable only from the main thread. Use the
/// [`on_audio_thread()`][Self::on_audio_thread()] method to spawn an audio thread.
#[derive(Debug)]
pub struct Plugin<'lib> {
    handle: PluginHandle,
    /// Information about this plugin instance stored on the host. This keeps track of things like
    /// audio thread IDs, whether the plugin has pending callbacks, and what state it is in.
    pub host_instance: Pin<Arc<HostPluginInstance>>,

    /// The CLAP plugin library this plugin instance was created from. This field is not used
    /// directly, but keeping a reference to the library here prevents the plugin instance from
    /// outliving the library.
    _library: &'lib PluginLibrary,
    /// To honor CLAP's thread safety guidelines, the thread this object was created from is
    /// designated the 'main thread', and this object cannot be shared with other threads. The
    /// [`on_audio_thread()`][Self::on_audio_thread()] method spawns an audio thread that is able to call
    /// the plugin's audio thread functions.
    _send_sync_marker: PhantomData<*const ()>,
}

/// The plugin's current lifecycle state. This is checked extensively to ensure that the plugin is
/// in the correct state, and things like double activations can't happen. `Plugin` and
/// `PluginAudioThread` will drop down to the previous state automatically when the object is
/// dropped and the stop processing or deactivate functions have not yet been calle.d
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PluginState {
    Deactivated,
    Activated,
    Processing,
}

/// An unsafe `Send` wrapper around [`Plugin`], needed to create the audio thread abstraction since
/// we artifically imposed `!Send`+`!Sync` on `Plugin` using the phantomdata marker.
struct PluginSendWrapper<'lib>(*const Plugin<'lib>);

unsafe impl<'lib> Send for PluginSendWrapper<'lib> {}

impl<'lib> Deref for PluginSendWrapper<'lib> {
    type Target = *const Plugin<'lib>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for Plugin<'_> {
    fn drop(&mut self) {
        match self
            .host_instance
            .state
            .compare_exchange(PluginState::Activated, PluginState::Deactivated)
        {
            Ok(_) => unsafe_clap_call! { self.as_ptr()=>deactivate(self.as_ptr()) },
            Err(PluginState::Deactivated) => (),
            Err(state) => panic!(
                "The plugin was in an invalid state '{state:?}' when the instance got dropped, \
                 this is a clap-validator bug"
            ),
        }

        // TODO: We can't handle host callbacks that happen in between these two functions, but the
        //       plugin really shouldn't be making callbacks in deactivate()
        unsafe_clap_call! { self.as_ptr()=>destroy(self.as_ptr()) };

        self.host_instance
            .host
            .unregister_instance(self.host_instance.clone());
    }
}

/// This allows methods from the CLAP plugin to be called directly independently of any
/// abstractions. All of the thread guarentees are lost when interacting with the plugin this way,
/// but that is not a problem as the function pointers are marked unsafe anyways.
impl Deref for Plugin<'_> {
    type Target = clap_plugin;

    fn deref(&self) -> &Self::Target {
        unsafe { self.handle.0.as_ref() }
    }
}

impl<'lib> Plugin<'lib> {
    /// Create a plugin instance and return the still uninitialized plugin. Returns an error if the
    /// plugin could not be created. The plugin instance will be registered with the host, and
    /// unregistered when this object is dropped again.
    pub fn new(
        library: &'lib PluginLibrary,
        host: Arc<ClapHost>,
        factory: &clap_plugin_factory,
        plugin_id: &CStr,
    ) -> Result<Self> {
        // The host can use this to keep track of things like audio threads and pending callbacks.
        // The instance is remvoed again when this object is dropped.
        let host_instance = HostPluginInstance::new(host.clone());
        let plugin = unsafe_clap_call! {
            factory=>create_plugin(factory, host_instance.as_ptr(), plugin_id.as_ptr())
        };
        if plugin.is_null() {
            anyhow::bail!(
                "'clap_plugin_factory::create_plugin({plugin_id:?})' returned a null pointer"
            );
        }

        // We can only register the plugin instance with the host now because we did not have a
        // plugin pointer before this.
        let handle = PluginHandle(NonNull::new(plugin as *mut clap_plugin).unwrap());
        host_instance.plugin.store(Some(handle));
        host.register_instance(host_instance.clone());

        Ok(Plugin {
            handle,
            host_instance,

            _library: library,
            _send_sync_marker: PhantomData,
        })
    }

    /// Get the raw pointer to the `clap_plugin` instance.
    pub fn as_ptr(&self) -> *const clap_plugin {
        self.handle.0.as_ptr()
    }

    /// Whether this plugin is currently active.
    pub fn activated(&self) -> bool {
        self.host_instance.state.load() >= PluginState::Activated
    }

    /// Get the _main thread_ extension abstraction for the extension `T`, if the plugin supports
    /// this extension. Returns `None` if it does not. The plugin needs to be initialized using
    /// [`init()`][Self::init()] before this may be called.
    pub fn get_extension<'a, T: Extension<&'a Self>>(&'a self) -> Option<T> {
        let extension_ptr = unsafe_clap_call! {
            self.as_ptr()=>get_extension(self.as_ptr(), T::EXTENSION_ID.as_ptr())
        };

        if extension_ptr.is_null() {
            None
        } else {
            Some(T::new(
                self,
                NonNull::new(extension_ptr as *mut T::Struct).unwrap(),
            ))
        }
    }

    /// Execute some code for this plugin from an audio thread context. The closure receives a
    /// [`PluginAudioThread`], which disallows calling main thread functions, and permits calling
    /// audio thread functions.
    ///
    /// TODO: Right now there's no way to interact with the main thread. This function should be
    ///       extended with a second closure to do main thread things.
    pub fn on_audio_thread<'a, T: Send, F: FnOnce(PluginAudioThread<'a>) -> T + Send>(
        &'a self,
        f: F,
    ) -> T {
        // This would be a hard mistake on the the validator's end, because th eaudio thread doesn't
        // exist when the plugin is deactivated.
        if !self.activated() {
            panic!(
                "'Plugin::on_audio_thread()' call while the plugin is not active, this is a bug \
                 in the validator."
            )
        }

        let unsafe_self_wrapper = PluginSendWrapper(self);
        crossbeam::scope(|s| {
            s.spawn(move |_| {
                // SAFETY: We artificially impose `!Send`+`!Sync` requirements on `Plugin` and
                //         `PluginAudioThread` to prevent them from being shared with other
                //         threads. But we'll need to temporarily lift that restriction in order
                //         to create this `PluginAudioThread`.
                let this = unsafe { &**unsafe_self_wrapper };

                // The host may use this to assert that calls are run from an audio thread
                this.host_instance
                    .audio_thread
                    .store(Some(std::thread::current().id()));
                let result = f(PluginAudioThread::new(this));
                this.host_instance.audio_thread.store(None);

                result
            })
            .join()
            .expect("Audio thread panicked")
        })
        .expect("Audio thread panicked")
    }

    /// Initialize the plugin. This needs to be called before doing anything else.
    pub fn init(&self) -> Result<()> {
        if unsafe_clap_call! { self.as_ptr()=>init(self.as_ptr()) } {
            Ok(())
        } else {
            anyhow::bail!("'clap_plugin::init()' returned false")
        }
    }

    /// Activate the plugin. Returns an error if the plugin returned `false`. See
    /// [plugin.h](https://github.com/free-audio/clap/blob/main/include/clap/plugin.h) for the
    /// preconditions.
    pub fn activate(
        &self,
        sample_rate: f64,
        min_buffer_size: usize,
        max_buffer_size: usize,
    ) -> Result<()> {
        match self
            .host_instance
            .state
            .compare_exchange(PluginState::Deactivated, PluginState::Activated)
        {
            Ok(_) => (),
            Err(PluginState::Activated) => {
                anyhow::bail!("Cannot activate an already active plugin.")
            }
            Err(state) => panic!(
                "Tried activating a plugin that was already processing audio ({state:?}), this is \
                 a clap-validator bug"
            ),
        }

        // Apparently 0 is invalid here
        assert!(min_buffer_size >= 1);

        if unsafe_clap_call! {
            self.as_ptr()=>activate(
                self.as_ptr(),
                sample_rate,
                min_buffer_size as u32,
                max_buffer_size as u32,
            )
        } {
            Ok(())
        } else {
            anyhow::bail!("'clap_plugin::activate()' returned false")
        }
    }

    /// Deactivate the plugin. See
    /// [plugin.h](https://github.com/free-audio/clap/blob/main/include/clap/plugin.h) for the
    /// preconditions.
    pub fn deactivate(&self) -> Result<()> {
        match self
            .host_instance
            .state
            .compare_exchange(PluginState::Activated, PluginState::Deactivated)
        {
            Ok(_) => (),
            Err(PluginState::Deactivated) => {
                anyhow::bail!("Cannot deactivate an inactive plugin.")
            }
            Err(state) => panic!(
                "Tried deactivating a plugin that was still processing audio ({state:?}), this is \
                 a clap-validator bug"
            ),
        }

        unsafe_clap_call! { self.as_ptr()=>deactivate(self.as_ptr()) };

        Ok(())
    }
}
