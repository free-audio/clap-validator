//! Abstractions for single CLAP plugin instances for main thread interactions.

use anyhow::Result;
use clap_sys::factory::plugin_factory::clap_plugin_factory;
use clap_sys::plugin::clap_plugin;
use std::ffi::CStr;
use std::marker::PhantomData;
use std::ops::Deref;
use std::pin::Pin;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;

use super::ext::Extension;
use super::library::{PluginLibrary, PluginMetadata};
use super::{assert_plugin_state_eq, assert_plugin_state_initialized};
use crate::plugin::host::{CallbackTask, Host, InstanceState};
use crate::util::unsafe_clap_call;
use audio_thread::PluginAudioThread;

pub mod audio_thread;
pub mod process;

/// A `Send+Sync` wrapper around `*const clap_plugin`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct PluginHandle(pub NonNull<clap_plugin>);

unsafe impl Send for PluginHandle {}
unsafe impl Sync for PluginHandle {}

/// A CLAP plugin instance. The plugin will be deinitialized when this object is dropped. All
/// functions here are callable only from the main thread. Use the
/// [`on_audio_thread()`][Self::on_audio_thread()] method to spawn an audio thread.
///
/// All functions on `Plugin` and the objects created from it will panic if the plugin is not in the
/// correct state.
#[derive(Debug)]
pub struct Plugin<'lib> {
    handle: PluginHandle,
    /// Information about this plugin instance stored on the host. This keeps track of things like
    /// audio thread IDs, whether the plugin has pending callbacks, and what state it is in.
    pub state: Pin<Arc<InstanceState>>,

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
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PluginStatus {
    #[default]
    Uninitialized,
    Deactivated,
    Activated,
    Processing,
}

/// An unsafe `Send` wrapper around [`Plugin`], needed to create the audio thread abstraction since
/// we artifically imposed `!Send`+`!Sync` on `Plugin` using the phantomdata marker.
struct PluginSendWrapper<'lib>(*const Plugin<'lib>);

unsafe impl<'lib> Send for PluginSendWrapper<'lib> {}

/// This `Deref` wrapper works around the !Sync check check we would interwise run into if we
/// accessed the struct's value directly.
impl<'lib> Deref for PluginSendWrapper<'lib> {
    type Target = *const Plugin<'lib>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for Plugin<'_> {
    fn drop(&mut self) {
        // Make sure the plugin is in the correct state before it gets destroyed
        match self.status() {
            PluginStatus::Uninitialized | PluginStatus::Deactivated => (),
            PluginStatus::Activated => self.deactivate(),
            status @ PluginStatus::Processing => panic!(
                "The plugin was in an invalid state '{status:?}' when the instance got dropped, \
                 this is a clap-validator bug"
            ),
        }

        // TODO: We can't handle host callbacks that happen in between these two functions, but the
        //       plugin really shouldn't be making callbacks in deactivate()
        let plugin = self.as_ptr();
        unsafe_clap_call! { plugin=>destroy(plugin) };

        self.host().unregister_instance(self.state.clone());
    }
}

impl<'lib> Plugin<'lib> {
    /// Create a plugin instance and return the still uninitialized plugin. Returns an error if the
    /// plugin could not be created. The plugin instance will be registered with the host, and
    /// unregistered when this object is dropped again.
    pub fn new(
        library: &'lib PluginLibrary,
        host: Rc<Host>,
        factory: &clap_plugin_factory,
        plugin_id: &CStr,
    ) -> Result<Self> {
        // The host can use this to keep track of things like audio threads and pending callbacks.
        // The instance is remvoed again when this object is dropped.
        let state = InstanceState::new(host.clone());
        let plugin = unsafe_clap_call! {
            factory=>create_plugin(factory, state.clap_host_ptr(), plugin_id.as_ptr())
        };
        if plugin.is_null() {
            anyhow::bail!(
                "'clap_plugin_factory::create_plugin({plugin_id:?})' returned a null pointer."
            );
        }

        // We can only register the plugin instance with the host now because we did not have a
        // plugin pointer before this.
        let handle = PluginHandle(NonNull::new(plugin as *mut clap_plugin).unwrap());
        state.plugin.store(Some(handle));
        host.register_instance(state.clone());

        Ok(Plugin {
            handle,
            state,

            _library: library,
            _send_sync_marker: PhantomData,
        })
    }

    /// Get the raw pointer to the `clap_plugin` instance.
    pub fn as_ptr(&self) -> *const clap_plugin {
        self.handle.0.as_ptr()
    }

    /// Get this plugin's metadata descriptor. In theory this should be the same as the one
    /// retrieved from the factory earlier.
    pub fn descriptor(&self) -> Result<PluginMetadata> {
        let plugin = self.as_ptr();
        let descriptor = unsafe { (*plugin).desc };
        if descriptor.is_null() {
            anyhow::bail!("The 'desc' field on the 'clap_plugin' struct is a null pointer.");
        }

        PluginMetadata::from_descriptor(unsafe { &*descriptor })
    }

    /// Get the host for this plugin instance.
    pub fn host(&self) -> &Host {
        // `Plugin` can only be used from the main thread
        self.state
            .host()
            .expect("Tried to get the host instance from a thread that isn't the main thread")
    }

    /// The plugin's current initialization status.
    pub fn status(&self) -> PluginStatus {
        self.state.status.load()
    }

    /// Get the _main thread_ extension abstraction for the extension `T`, if the plugin supports
    /// this extension. Returns `None` if it does not. The plugin needs to be initialized using
    /// [`init()`][Self::init()] before this may be called.
    pub fn get_extension<'a, T: Extension<&'a Self>>(&'a self) -> Option<T> {
        assert_plugin_state_initialized!(self);

        let plugin = self.as_ptr();
        let extension_ptr = unsafe_clap_call! {
            plugin=>get_extension(plugin, T::EXTENSION_ID.as_ptr())
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
    /// If whatever happens on the audio thread caused main-thread callback requests to be emited,
    /// then those will be handled concurrently.
    pub fn on_audio_thread<'a, T: Send, F: FnOnce(PluginAudioThread<'a>) -> T + Send>(
        &'a self,
        f: F,
    ) -> T {
        assert_plugin_state_eq!(self, PluginStatus::Activated);

        crossbeam::scope(|s| {
            let unsafe_self_wrapper = PluginSendWrapper(self);
            let callback_task_sender = self.host().callback_task_sender.clone();

            let audio_thread = s
                .builder()
                .name(String::from("audio-thread"))
                .spawn(move |_| {
                    // SAFETY: We artificially impose `!Send`+`!Sync` requirements on `Plugin` and
                    //         `PluginAudioThread` to prevent them from being shared with other
                    //         threads. But we'll need to temporarily lift that restriction in order
                    //         to create this `PluginAudioThread`.
                    let this = unsafe { &**unsafe_self_wrapper };

                    // The host may use this to assert that calls are run from an audio thread
                    this.state
                        .audio_thread
                        .store(Some(std::thread::current().id()));
                    let result = f(PluginAudioThread::new(this));
                    this.state.audio_thread.store(None);

                    // The main thread should unblock when the audio thread is done
                    callback_task_sender.send(CallbackTask::Stop).unwrap();

                    result
                })
                .expect("Unable to spawn an audio thread");

            // Handle callbacks requests on the main thread whle the aduio thread is running
            self.host().handle_callbacks_blocking();

            audio_thread.join().expect("Audio thread panicked")
        })
        .expect("Audio thread panicked")
    }

    /// Initialize the plugin. This needs to be called before doing anything else.
    pub fn init(&self) -> Result<()> {
        assert_plugin_state_eq!(self, PluginStatus::Uninitialized);

        let plugin = self.as_ptr();
        if unsafe_clap_call! { plugin=>init(plugin) } {
            self.state.status.store(PluginStatus::Deactivated);
            Ok(())
        } else {
            anyhow::bail!("'clap_plugin::init()' returned false.")
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
        assert_plugin_state_eq!(self, PluginStatus::Deactivated);

        // Apparently 0 is invalid here
        assert!(min_buffer_size >= 1);

        let plugin = self.as_ptr();
        if unsafe_clap_call! {
            plugin=>activate(plugin, sample_rate, min_buffer_size as u32, max_buffer_size as u32)
        } {
            self.state.status.store(PluginStatus::Activated);
            Ok(())
        } else {
            anyhow::bail!("'clap_plugin::activate()' returned false.")
        }
    }

    /// Deactivate the plugin. See
    /// [plugin.h](https://github.com/free-audio/clap/blob/main/include/clap/plugin.h) for the
    /// preconditions.
    pub fn deactivate(&self) {
        assert_plugin_state_eq!(self, PluginStatus::Activated);

        let plugin = self.as_ptr();
        unsafe_clap_call! { plugin=>deactivate(plugin) };

        self.state.status.store(PluginStatus::Deactivated);
    }
}
