//! Abstractions for single CLAP plugin instances for main thread interactions.

use anyhow::Result;
use clap_sys::plugin::clap_plugin;
use std::marker::PhantomData;
use std::ops::Deref;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::Arc;
use std::thread::ThreadId;

use super::audio_thread::PluginAudioThread;
use super::ext::Extension;
use super::library::PluginLibrary;
use crate::hosting::ClapHost;

/// A CLAP plugin instance. The plugin will be deinitialized when this object is dropped. All
/// functions here are callable only from the main thread. Use the
/// [`on_audio_thread()`][Self::on_audio_thread()] method to spawn an audio thread.
#[derive(Debug)]
pub struct Plugin<'lib> {
    handle: NonNull<clap_plugin>,
    /// The ID of the main thread. Or in other words, the ID of the thread this `Plugin` instance
    /// was created from. This is useful when working with audio threads. We want the audio thread
    /// to be separate from the main thread, but in some test cases it may be useful to process
    /// multiple plugin instances in series from the same audio thread. Because of this,
    /// [`Plugin::on_audio_thread()`] checks whether the function is called from the main thread or
    /// not. If it is, then a new thread is spawned and the closure is run from that thread. If the
    /// function is called from another thread, then the closure can be run directly.
    main_thread_id: ThreadId,
    /// The CLAP plugin library this plugin instance was created from. This field is not used
    /// directly, but keeping a reference to the library here prevents the plugin instance from
    /// outliving the library.
    _library: &'lib PluginLibrary,
    /// The host instance for this plugin. Depending on the test, different instances may get their
    /// own host, or they can share a single host instance.
    _host: Pin<Arc<ClapHost>>,
    /// To honor CLAP's thread safety guidelines, the thread this object was created from is
    /// designated the 'main thread', and this object cannot be shared with other threads. The
    /// [`on_audio_thread()`][Self::on_audio_thread()] method spawns an audio thread that is able to call
    /// the plugin's audio thread functions.
    _send_sync_marker: PhantomData<*const ()>,
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
        unsafe { (self.handle.as_ref().destroy)(self.as_ptr()) };
    }
}

/// This allows methods from the CLAP plugin to be called directly independently of any
/// abstractions. All of the thread guarentees are lost when interacting with the plugin this way,
/// but that is not a problem as the function pointers are marked unsafe anyways.
impl Deref for Plugin<'_> {
    type Target = clap_plugin;

    fn deref(&self) -> &Self::Target {
        unsafe { self.handle.as_ref() }
    }
}

impl<'lib> Plugin<'lib> {
    pub fn new(
        handle: NonNull<clap_plugin>,
        library: &'lib PluginLibrary,
        host: Pin<Arc<ClapHost>>,
    ) -> Self {
        Plugin {
            handle,
            main_thread_id: std::thread::current().id(),
            _library: library,
            _host: host,
            _send_sync_marker: PhantomData,
        }
    }

    /// Get the raw pointer to the `clap_plugin` instance.
    pub fn as_ptr(&self) -> *const clap_plugin {
        self.handle.as_ptr()
    }

    /// Get the _main thread_ extension abstraction for the extension `T`, if the plugin supports
    /// this extension. Returns `None` if it does not. The plugin needs to be initialized using
    /// [`init()`][Self::init()] before this may be called.
    pub fn get_extension<'a, T: Extension<&'a Self>>(&'a self) -> Option<T> {
        let extension_ptr =
            unsafe { (self.handle.as_ref().get_extension)(self.as_ptr(), T::EXTENSION_ID) };

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
    /// audio thread functions. If this function is called from the main thread (the thread where
    /// the plugin instance was created on), then this closure will be run from a new thread. If
    /// this function is called from another thread, then the closure is run directly.
    pub fn on_audio_thread<'a, T: Send, F: FnOnce(PluginAudioThread<'a>) -> T + Send>(
        &'a self,
        f: F,
    ) -> T {
        if std::thread::current().id() == self.main_thread_id {
            let unsafe_self_wrapper = PluginSendWrapper(self);

            crossbeam::scope(|s| {
                s.spawn(move |_| {
                    // SAFETY: We artificially impose `!Send`+`!Sync` requirements on `Plugin` and
                    //         `PluginAudioThread` to prevent them from being shared with other
                    //         threads. But we'll need to temporarily lift that restriction in order
                    //         to create this `PluginAudioThread`.
                    let this = unsafe { &**unsafe_self_wrapper };

                    f(PluginAudioThread::new(this))
                })
                .join()
                .expect("Audio thread panicked")
            })
            .expect("Audio thread panicked")
        } else {
            f(PluginAudioThread::new(self))
        }
    }

    /// Initialize the plugin. This needs to be called before doing anything else.
    pub fn init(&self) -> Result<()> {
        if unsafe { (self.handle.as_ref().init)(self.as_ptr()) } {
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
        if unsafe {
            (self.handle.as_ref().activate)(
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
    pub fn deactivate(&self) {
        unsafe { (self.handle.as_ref().deactivate)(self.as_ptr()) };
    }
}
