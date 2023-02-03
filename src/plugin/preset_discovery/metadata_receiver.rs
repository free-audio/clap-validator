//! The metadata abstraction for a CLAP plugin's preset discovery factory. This is used when
//! querying metadata for a plugin's file. This is sort of like a state machine the plugin writes
//! one or more presets to.

use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use std::borrow::BorrowMut;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::{c_char, c_void, CString};
use std::path::PathBuf;
use std::pin::Pin;
use std::thread::ThreadId;

use clap_sys::factory::draft::preset_discovery::{
    clap_preset_discovery_filetype, clap_preset_discovery_indexer, clap_preset_discovery_location,
    clap_preset_discovery_metadata_receiver, clap_preset_discovery_soundpack,
    CLAP_PRESET_DISCOVERY_IS_DEMO_CONTENT, CLAP_PRESET_DISCOVERY_IS_FACTORY_CONTENT,
    CLAP_PRESET_DISCOVERY_IS_FAVORITE, CLAP_PRESET_DISCOVERY_IS_USER_CONTENT,
    CLAP_TIMESTAMP_UNKNOWN,
};
use clap_sys::version::CLAP_VERSION;
use parking_lot::Mutex;

use crate::util::{self, check_null_ptr};

/// An implementation of the preset discovery's metadata receiver. This borrows a
/// `Result<PresetFile>` because the important work is done when this object is dropped. When this
/// object is dropped, that result will contain either an error, a single preset, or a container of
/// presets depending on what the plugin declared and if there were any errors. This object contains
/// an internal preset list to be able to support both single presets and container files. The
/// lifecycle works as follows:
///
/// - Whenever the plugin calls `clap_preset_discovery_metadata_receiver::begin_preset()` a second
///   time (with a load key), or when the object is dropped, the data set on this object is written
///   to a pending preset vector.
/// - When this object is dropped, the `Result<PresetFile>` is written to as described above.
///
/// IO errors returned by the plugin are treated as hard errors for now.
#[derive(Debug)]
pub struct MetadataReceiver<'a> {
    /// The thread ID for the thread this object was created on. This object is not thread-safe, so
    /// we'll assert that all callbacks are made from this thread.
    expected_thread_id: ThreadId,

    /// See this object's docstring. If an error occurs, then the error is written here immediately.
    /// If the object is dropped and all presets have been written to `pending_presets` without any
    /// errors occurring, then this will contain a [`PresetFile`] describing the preset(s) added by
    /// the plugin.
    ///
    /// Stored in a `RefCell` in the off chance that the plugin doesn't use this in a thread safe
    /// way.
    result: RefCell<&'a mut Option<Result<PresetFile>>>,

    /// The data for the next preset. This is `None` until the plugin starts calling one of the data
    /// setter functions. After that point the preset's data is filled in piece by piece like in a
    /// state machine. This is finally transformed into a [`Preset`] when the object is dropped or
    /// when the plugin declares another preset.
    ///
    /// The object is instantiated when `begin_preset()` gets called. This also ensures that the
    /// plugin doesn't forget to call the function.
    next_preset_data: RefCell<Option<PartialPreset>>,
    /// The load key for the next container preset. Set during the first `begin_preset()` call based
    /// on the presence of `load_key`. If this is not set, then subsequent `begin_preset()` calls
    /// are treated as errors. Used in `maybe_write_preset()`.
    next_load_key: RefCell<Option<String>>,

    /// The vtable that's passed to the provider. The `receiver_data` field is populated with a
    /// pointer to this object.
    clap_preset_discovery_metadata_receiver: Mutex<clap_preset_discovery_metadata_receiver>,
}

/// One or more presets declared by the plugin through a preset provider metadata receiver.
#[derive(Debug, Clone)]
pub enum PresetFile {
    Single(Preset),
    /// This contains one or more presets with associated load keys.
    Container(BTreeMap<String, Preset>),
}

/// Data for the next preset. This is added bit by bit through the methods on the
/// `clap_preset_discovery_metadata_receiver`. The object gets transformed into a [`Preset`] when
/// the [`MetadataReceiver`] is dropped or when `begin_preset()` is called a second time.
#[derive(Debug, Clone, Default)]
struct PartialPreset {
    pub name: String,
    pub plugin_id: Option<String>,
    pub soundpack_id: Option<String>,
    /// These may remain unset, in which case the host should inherit them from the location.
    pub is_factory_content: Option<bool>,
    pub is_user_content: Option<bool>,
    pub is_demo_content: Option<bool>,
    pub is_favorite: Option<bool>,
    pub creator: Vec<String>,
    pub description: Option<String>,
    pub creation_time: Option<DateTime<Utc>>,
    pub modification_time: Option<DateTime<Utc>>,
    pub features: Vec<String>,
    pub extra_info: BTreeMap<String, String>,
}

impl PartialPreset {
    /// Convert this data to a preset. Returns an error if any data is missing. Individual fields
    /// will have already been validated before it was stored on this `PartialPreset`.
    pub fn finalize(self) -> Result<Preset> {
        todo!()
    }
}

#[derive(Debug, Clone)]
pub struct Preset {
    pub name: String,
    pub plugin_id: String,
    pub soundpack_id: Option<String>,
    /// These may remain unset, in which case the host should inherit them from the location.
    pub is_factory_content: Option<bool>,
    pub is_user_content: Option<bool>,
    pub is_demo_content: Option<bool>,
    pub is_favorite: Option<bool>,
    pub creator: Vec<String>,
    pub description: Option<String>,
    pub creation_time: Option<DateTime<Utc>>,
    pub modification_time: Option<DateTime<Utc>>,
    pub features: Vec<String>,
    pub extra_info: BTreeMap<String, String>,
}

impl Drop for MetadataReceiver<'_> {
    fn drop(&mut self) {
        // If the plugin declared a(nother) preset file, then this will be added to `self.result`
        // now. If an error occurred at any point, then the result will instead contain that error.
        self.maybe_write_preset();
    }
}

impl<'a> MetadataReceiver<'a> {
    /// Create a new metadata receiver that will write the results to the provided `result`. This is
    /// needed because the actual writing happens when this object is dropped. After that point
    /// `result` is either:
    ///
    /// - `None` if the plugin didn't write any presets.
    /// - `Some(Err(err))` if an error occurred while declaring presets.
    /// - `Some(Ok(preset_file))` if the plugin declared one or more presets successfully.
    pub fn new(result: &'a mut Option<Result<PresetFile>>) -> Pin<Box<Self>> {
        // In the event that the caller reuses result objects this needs to be initialized to a
        // non-error value, since if it does contain an error at some point then nothing will be
        // written to it in the `Drop` implementation
        *result = None;

        let metadata_receiver = Box::pin(Self {
            expected_thread_id: std::thread::current().id(),

            result: RefCell::new(result),
            next_preset_data: RefCell::new(None),
            next_load_key: RefCell::new(None),

            clap_preset_discovery_metadata_receiver: Mutex::new(
                clap_preset_discovery_metadata_receiver {
                    // This is set to a pointer to this pinned data structure later
                    receiver_data: std::ptr::null_mut(),
                    on_error: todo!(),
                    begin_preset: todo!(),
                    add_plugin_id: todo!(),
                    set_soundpack_id: todo!(),
                    set_flags: todo!(),
                    add_creator: todo!(),
                    set_description: todo!(),
                    set_timestamps: todo!(),
                    add_feature: todo!(),
                    add_extra_info: todo!(),
                },
            ),
        });

        metadata_receiver
            .clap_preset_discovery_metadata_receiver
            .lock()
            .receiver_data = &*metadata_receiver as *const Self as *mut c_void;

        metadata_receiver
    }

    /// Get a `clap_preset_discovery_metadata_receiver` vtable pointer that can be passed to the
    /// `clap_preset_discovery_factory` when creating a provider.
    pub fn clap_preset_discovery_metadata_receiver_ptr(
        self: &Pin<Box<Self>>,
    ) -> *const clap_preset_discovery_metadata_receiver {
        self.clap_preset_discovery_metadata_receiver.data_ptr()
    }

    /// Checks that this function is called from the same thread the indexer was created on. If it
    /// is not, then an error indicating this can be retrieved using
    /// [`callback_error_check()`][Self::callback_error_check()]. Subsequent thread safety errors
    /// will not overwrite earlier ones.
    fn assert_same_thread(&self, function_name: &str) {
        let current_thread_id = std::thread::current().id();
        if current_thread_id != self.expected_thread_id {
            self.set_callback_error(format!(
                "'{}' may only be called from the same thread the 'clap_preset_indexer' was \
                 created on (thread {:?}), but it was called from thread {:?}",
                function_name, self.expected_thread_id, current_thread_id
            ));
        }
    }

    /// Write an error to the result field if it did not already contain a value. Earlier errors are
    /// not overwritten.
    fn set_callback_error(&self, error: String) {
        match &mut *self.result.borrow_mut() {
            Some(Err(_)) => (),
            result => **result = Some(Err(anyhow::anyhow!(error))),
        }
    }

    /// If `self.next_preset_data` is non-empty, then transform the data into a [`Preset`] and write
    /// it to a [`PresetFile`] stored in `self.result`. This is a single file or a container file
    /// depending on whether a load key was passed to the `begin_preset()` function. If multiple
    /// presets are written for a single-file preset, then an error will be written to the result.
    /// If an error was previously written, then it will not be overwritten.
    fn maybe_write_preset(&self) {
        if let Some(partial_preset) = self.next_preset_data.borrow_mut().take() {
            match (
                &mut *self.result.borrow_mut(),
                partial_preset.finalize(),
                // The `take()` is important here to catch the situation where the plugin adds a
                // load key on the first `begin_preset()` call but not in subsequent calls
                self.next_load_key.borrow_mut().take(),
            ) {
                // If an error was already produced then it should be preserved, and new errors
                // should be written to the Result if there wasn't already one
                (Some(Err(_)), _, _) => (),
                (_, Err(err), _) => self.set_callback_error(format!("{err:#}")),
                (result @ None, Ok(preset), None) => {
                    **result = Some(Ok(PresetFile::Single(preset)))
                }
                (result @ None, Ok(preset), Some(load_key)) => {
                    let mut presets = BTreeMap::new();
                    presets.insert(load_key, preset);

                    **result = Some(Ok(PresetFile::Container(presets)));
                }
                (Some(Ok(PresetFile::Container(presets))), Ok(preset), Some(load_key)) => {
                    presets.insert(load_key, preset);
                }
                // These situations have been caught in `begin_preset()`. If a second preset has
                // been started when the first preset didn't have a load key this is a validator
                // bug.
                (Some(Ok(PresetFile::Single(_))), Ok(_), _)
                | (Some(Ok(PresetFile::Container(_))), Ok(_), None) => unreachable!(
                    "Inconsistent state in the validator's metadata receiver found, this is a \
                     clap-validator bug."
                ),
            }
        }
    }
}
