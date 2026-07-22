use crate::cli::tracing::{Span, record};
use crate::plugin::ext::Extension;
use crate::plugin::instance::PluginShared;
use crate::plugin::util::clap_call;
use clap_sys::ext::thread_pool::{CLAP_EXT_THREAD_POOL, clap_plugin_thread_pool};
use std::ffi::CStr;
use std::ptr::NonNull;

#[derive(Clone, Copy)]
pub struct ThreadPool<'a> {
    plugin: &'a PluginShared,
    thread_pool: NonNull<clap_plugin_thread_pool>,
}

unsafe impl Send for ThreadPool<'_> {}
unsafe impl Sync for ThreadPool<'_> {}

impl<'a> Extension for ThreadPool<'a> {
    const IDS: &'static [&'static CStr] = &[CLAP_EXT_THREAD_POOL];

    type Plugin = &'a PluginShared;
    type Struct = clap_plugin_thread_pool;

    unsafe fn new(plugin: &'a PluginShared, thread_pool: NonNull<Self::Struct>) -> Self {
        Self { plugin, thread_pool }
    }
}

impl<'a> ThreadPool<'a> {
    pub fn exec(&self, task: u32) {
        let thread_pool = self.thread_pool.as_ptr();
        let plugin = self.plugin.clap_plugin;

        let _span = Span::begin("clap_plugin_thread_pool::exec", record! { task: task });
        unsafe {
            clap_call! { thread_pool=>exec(plugin, task) }
        }
    }
}
