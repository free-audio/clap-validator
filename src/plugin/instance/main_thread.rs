use crate::cli::tracing::{Span, record};
use crate::plugin::ext::Extension;
use crate::plugin::instance::{CallbackEvent, PluginAudioThread, PluginShared, PluginStatus};
use crate::plugin::library::PluginMetadata;
use crate::plugin::util::{Proxy, clap_call};
use anyhow::Result;
use clap_sys::plugin::clap_plugin;
use std::marker::PhantomData;
use std::panic::resume_unwind;
use std::sync::mpsc::Receiver;

/// A CLAP plugin instance. The plugin will be deinitialized when this object is dropped. All
/// functions here are callable only from the main thread. Use the
/// [`on_audio_thread()`][Self::on_audio_thread()] method to spawn an audio thread.
///
/// All functions on `Plugin` and the objects created from it will panic if the plugin is not in the
/// correct state.
pub struct Plugin<'lib> {
    pub(super) callback_receiver: Receiver<CallbackEvent>,

    /// Information about this plugin instance stored on the host. This keeps track of things like
    /// audio thread IDs, whether the plugin has pending callbacks, and what state it is in.
    pub(super) shared: Proxy<PluginShared>,

    /// The CLAP plugin library this plugin instance was created from. This field is not used
    /// directly, but keeping a reference to the library here prevents the plugin instance from
    /// outliving the library.
    pub(super) _library: PhantomData<&'lib ()>,

    /// To honor CLAP's thread safety guidelines, the thread this object was created from is
    /// designated the 'main thread', and this object cannot be shared with other threads. The
    /// [`on_audio_thread()`][Self::on_audio_thread()] method spawns an audio thread that is able to call
    /// the plugin's audio thread functions.
    pub(super) _thread: PhantomData<*const ()>,
}

impl Drop for Plugin<'_> {
    fn drop(&mut self) {
        if let Some(error) = self.shared.callback_error.lock().unwrap().take() {
            log::warn!(
                "The validator's host has detected a callback error but this error has not been used as part of the \
                 test result. This could be a clap-validator bug. The error message is: {error}"
            )
        }

        // Make sure the plugin is in the correct state before it gets destroyed
        match self.status() {
            PluginStatus::Uninitialized | PluginStatus::Deactivated => (),
            status => panic!(
                "The plugin was in an invalid state '{status:?}' when the instance got dropped, this is a \
                 clap-validator bug"
            ),
        }

        let plugin = self.as_ptr();
        unsafe {
            let _span = Span::begin("clap_plugin::destroy", ());
            clap_call! { plugin=>destroy(plugin) }
        }
    }
}

impl<'lib> Plugin<'lib> {
    /// Get the raw pointer to the `clap_plugin` instance.
    pub fn as_ptr(&self) -> *const clap_plugin {
        self.shared.clap_plugin
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

    /// The plugin's current initialization status.
    pub fn status(&self) -> PluginStatus {
        self.shared.status()
    }

    /// Handle any pending main-thread callbacks for this plugin and pending callback events.
    /// Returns an error if a callback error occurred.
    pub fn poll_callback(&self, mut f: impl FnMut(CallbackEvent) -> Result<()>) -> Result<()> {
        self.poll_callback_unchecked();

        if let Some(error) = self.shared.callback_error.lock().unwrap().take() {
            anyhow::bail!(error);
        }

        while let Ok(event) = self.callback_receiver.try_recv() {
            f(event)?;
        }

        Ok(())
    }

    /// Get the _main thread_ extension abstraction for the extension `T`, if the plugin supports
    /// this extension. Returns `None` if it does not. The plugin needs to be initialized using
    /// [`init()`][Self::init()] before this may be called.
    pub fn get_extension<'a, T: Extension<Plugin = &'a Self>>(&'a self) -> Option<T> {
        unsafe { self.shared.raw_extension::<T>().map(|ptr| T::new(self, ptr)) }
    }

    /// Execute some code for this plugin from an audio thread context. The closure receives a
    /// [`PluginAudioThread`], which disallows calling main thread functions, and permits calling
    /// audio thread functions.
    ///
    /// If whatever happens on the audio thread caused main-thread callback requests to be emited,
    /// then those will be handled concurrently.
    pub fn on_audio_thread<T: Send, F: FnOnce(PluginAudioThread) -> Result<T> + Send>(&self, f: F) -> Result<T> {
        if self.shared.audio_thread_id.load().is_some() {
            panic!("An audio thread is already running for this plugin instance.");
        }

        let (sender, receiver) = std::sync::mpsc::channel();

        let result = std::thread::scope(|s| {
            let shared = self.shared.clone();
            let audio_thread = std::thread::Builder::new()
                .name("audio".into())
                .spawn_scoped(s, || f(PluginAudioThread::new(shared, sender)))
                .unwrap();

            let mut error = None;
            while let Ok(task) = receiver.recv() {
                if let Err(e) = task(self) {
                    error.get_or_insert(e);
                }
            }

            if let Some(error) = error {
                return Err(error);
            }

            audio_thread.join().unwrap_or_else(|e| resume_unwind(e))
        });

        self.poll_callback_unchecked();
        result
    }

    /// Initialize the plugin. This needs to be called before doing anything else.
    pub fn init(&self) -> Result<()> {
        self.status().assert_is(PluginStatus::Uninitialized);
        self.shared.set_status(PluginStatus::Initializing);

        let _span = Span::begin("clap_plugin::init", ());
        let result = unsafe {
            clap_call! { self.as_ptr()=>init(self.as_ptr()) }
        };

        if result {
            // If the plugin never calls `request_callback`, the validator won't catch this
            anyhow::ensure!(
                unsafe { (*self.as_ptr()).on_main_thread.is_some() },
                "clap_plugin::on_main_thread is null"
            );

            self.shared.set_status(PluginStatus::Deactivated);
            Ok(())
        } else {
            self.shared.set_status(PluginStatus::Uninitialized);
            anyhow::bail!("'clap_plugin::init()' returned false.")
        }
    }

    /// Activate the plugin. Returns an error if the plugin returned `false`. See
    /// [plugin.h](https://github.com/free-audio/clap/blob/main/include/clap/plugin.h) for the
    /// preconditions.
    ///
    /// Also checks for 'activate'/'request_restart' loops.
    pub fn activate(&self, sample_rate: f64, min_buffer_size: u32, max_buffer_size: u32) -> Result<()> {
        self.status().assert_is(PluginStatus::Deactivated);

        // Apparently 0 is invalid here
        assert!(min_buffer_size >= 1);
        assert!(max_buffer_size >= min_buffer_size);

        for i in (0..10).rev() {
            // we need to track the `Activating` state to validate that we call clap_host_latency::changed only within the activation call.
            self.shared.set_status(PluginStatus::Activating);

            let result = unsafe {
                let span = Span::begin(
                    "clap_plugin::activate",
                    record! {
                        sample_rate: sample_rate,
                        min_buffer_size: min_buffer_size,
                        max_buffer_size: max_buffer_size
                    },
                );

                let result = clap_call! { self.as_ptr()=>activate(self.as_ptr(), sample_rate, min_buffer_size, max_buffer_size) };
                span.finish(record!(result: result));
                result
            };

            if result {
                self.shared.set_status(PluginStatus::Activated);
            } else {
                self.shared.set_status(PluginStatus::Deactivated);
                anyhow::bail!("'clap_plugin::activate()' returned false.")
            }

            if self.shared.requested_restart.swap(false) {
                if i == 0 {
                    anyhow::bail!("The plugin seems to be stuck in an 'activate'/'request_restart' loop");
                } else {
                    self.deactivate();
                    continue;
                }
            }

            return Ok(());
        }

        Ok(())
    }

    /// Deactivate the plugin. See
    /// [plugin.h](https://github.com/free-audio/clap/blob/main/include/clap/plugin.h) for the
    /// preconditions.
    pub fn deactivate(&self) {
        self.status().assert_is(PluginStatus::Activated);

        unsafe {
            let _span = Span::begin("clap_plugin::deactivate", ());
            clap_call! { self.as_ptr()=>deactivate(self.as_ptr()) }
        }

        self.shared.set_status(PluginStatus::Deactivated);
    }

    /// Same as [`poll_callback()`][Self::poll_callback()] but does not check for callback errors, and does not process callback events.
    pub fn poll_callback_unchecked(&self) {
        // 10 iterations, then bail
        for _ in 0..10 {
            if !self.shared.requested_callback.swap(false) {
                return;
            }

            unsafe {
                let _span = Span::begin("clap_plugin::on_main_thread", ());
                clap_call! { self.as_ptr()=>on_main_thread(self.as_ptr()) }
            };
        }

        log::warn!("The plugin seems to be stuck in an 'on_main_thread' callback loop");
    }
}
