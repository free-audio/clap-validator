//! The metadata abstraction for a CLAP plugin's preset discovery factory. This is used when
//! querying metadata for a plugin's file. This is sort of like a state machine the plugin writes
//! one or more presets to.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use clap_sys::factory::draft::preset_discovery::{
    clap_plugin_id, clap_preset_discovery_metadata_receiver, clap_timestamp,
    CLAP_PRESET_DISCOVERY_IS_DEMO_CONTENT, CLAP_PRESET_DISCOVERY_IS_FACTORY_CONTENT,
    CLAP_PRESET_DISCOVERY_IS_FAVORITE, CLAP_PRESET_DISCOVERY_IS_USER_CONTENT,
};
use parking_lot::Mutex;
use serde::Serialize;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::{c_char, c_void};
use std::pin::Pin;
use std::thread::ThreadId;

use super::{Flags, Location};
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

    /// The crawled location's flags. This is used as a fallback for the preset flags if the
    /// provider does not explicitly set flags for a preset.
    location_flags: Flags,
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
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PresetFile {
    Single(Preset),
    /// This contains one or more presets with associated load keys.
    Container(BTreeMap<String, Preset>),
}

/// Data for the next preset. This is added bit by bit through the methods on the
/// `clap_preset_discovery_metadata_receiver`. The object gets transformed into a [`Preset`] when
/// the [`MetadataReceiver`] is dropped or when `begin_preset()` is called a second time.
#[derive(Debug, Clone)]
struct PartialPreset {
    pub name: String,
    pub plugin_ids: Vec<PluginId>,
    pub soundpack_id: Option<String>,
    /// These may remain unset, in which case the host should inherit them from the location
    pub flags: Option<Flags>,
    pub creators: Vec<String>,
    pub description: Option<String>,
    pub creation_time: Option<DateTime<Utc>>,
    pub modification_time: Option<DateTime<Utc>>,
    pub features: Vec<String>,
    pub extra_info: BTreeMap<String, String>,
}

impl PartialPreset {
    pub fn new(name: String) -> Self {
        Self {
            name,
            plugin_ids: Default::default(),
            soundpack_id: Default::default(),
            flags: Default::default(),
            creators: Default::default(),
            description: Default::default(),
            creation_time: Default::default(),
            modification_time: Default::default(),
            features: Default::default(),
            extra_info: Default::default(),
        }
    }

    /// Convert this data to a preset. Returns an error if any data is missing. Individual fields
    /// will have already been validated before it was stored on this `PartialPreset`. If there were
    /// no flags set for this preset, then the location's flags will be used.
    pub fn finalize(self, location_flags: &Flags) -> Result<Preset> {
        if self.plugin_ids.is_empty() {
            anyhow::bail!(
                "The preset '{}' was defined without setting a plugin ID.",
                self.name
            );
        }

        Ok(Preset {
            name: self.name,
            plugin_ids: self.plugin_ids,
            soundpack_id: self.soundpack_id,
            flags: match self.flags {
                Some(flags) => PresetFlags::Explicit(flags),
                None => PresetFlags::Inherited(*location_flags),
            },
            creators: self.creators,
            description: self.description,
            creation_time: self.creation_time,
            modification_time: self.modification_time,
            features: self.features,
            extra_info: self.extra_info,
        })
    }
}

/// The plugin ABI the preset was defined for. Most plugins will define only presets for CLAP
/// plugins.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct PluginId {
    #[serde(serialize_with = "plugin_abi_to_string")]
    pub abi: PluginAbi,
    pub id: String,
}

/// Always serialize this as a string. Having the `Other` enum variant is nice but it looks out of
/// place in the JSON output.
fn plugin_abi_to_string<S>(plugin_abi: &PluginAbi, ser: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match plugin_abi {
        PluginAbi::Clap => "clap".serialize(ser),
        PluginAbi::Other(s) => s.serialize(ser),
    }
}

/// The plugin ABI the preset was defined for. Most plugins will define only presets for CLAP
/// plugins.
#[derive(Debug, Clone, PartialEq)]
pub enum PluginAbi {
    Clap,
    Other(String),
}

/// A preset as declared by the plugin. Constructed from a [`PartialPreset`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Preset {
    pub name: String,
    pub plugin_ids: Vec<PluginId>,
    pub soundpack_id: Option<String>,
    pub flags: PresetFlags,
    pub creators: Vec<String>,
    pub description: Option<String>,
    pub creation_time: Option<DateTime<Utc>>,
    pub modification_time: Option<DateTime<Utc>>,
    pub features: Vec<String>,
    pub extra_info: BTreeMap<String, String>,
}

/// The flags applying to a preset. These are either explicitly set for the preset or inherited from
/// the location.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "kebab-case", tag = "type")]
pub enum PresetFlags {
    /// The fall back to the location's flags if the provider did not explicitly set flags for the
    /// preset.
    Inherited(Flags),
    /// Flags that were explicitly set for the preset.
    Explicit(Flags),
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
    pub fn new(result: &'a mut Option<Result<PresetFile>>, location: &Location) -> Pin<Box<Self>> {
        // In the event that the caller reuses result objects this needs to be initialized to a
        // non-error value, since if it does contain an error at some point then nothing will be
        // written to it in the `Drop` implementation
        *result = None;

        let metadata_receiver = Box::pin(Self {
            expected_thread_id: std::thread::current().id(),

            location_flags: location.flags,
            result: RefCell::new(result),
            next_preset_data: RefCell::new(None),
            next_load_key: RefCell::new(None),

            clap_preset_discovery_metadata_receiver: Mutex::new(
                clap_preset_discovery_metadata_receiver {
                    // This is set to a pointer to this pinned data structure later
                    receiver_data: std::ptr::null_mut(),
                    on_error: Some(Self::on_error),
                    begin_preset: Some(Self::begin_preset),
                    add_plugin_id: Some(Self::add_plugin_id),
                    set_soundpack_id: Some(Self::set_soundpack_id),
                    set_flags: Some(Self::set_flags),
                    add_creator: Some(Self::add_creator),
                    set_description: Some(Self::set_description),
                    set_timestamps: Some(Self::set_timestamps),
                    add_feature: Some(Self::add_feature),
                    add_extra_info: Some(Self::add_extra_info),
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
    fn set_callback_error(&self, error: impl Into<String>) {
        match &mut *self.result.borrow_mut() {
            Some(Err(_)) => (),
            result => **result = Some(Err(anyhow::anyhow!(error.into()))),
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
                partial_preset.finalize(&self.location_flags),
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

    unsafe extern "C" fn on_error(
        receiver: *const clap_preset_discovery_metadata_receiver,
        os_error: i32,
        error_message: *const c_char,
    ) {
        // We'll have a dedicated error message for a missing `error_message`
        check_null_ptr!(receiver, (*receiver).receiver_data);
        let this = &*((*receiver).receiver_data as *const Self);

        this.assert_same_thread("clap_preset_discovery_metadata_receiver::on_error()");

        let error_message = unsafe { util::cstr_ptr_to_mandatory_string(error_message) }.context(
            "'clap_preset_discovery_metadata_receiver::on_error()' called with an invalid error \
             message",
        );
        match error_message {
            Ok(error_message) => this.set_callback_error(format!(
                "'clap_preset_discovery_metadata_receiver::on_error()' called for OS error code \
                 {os_error} with the following error message: {error_message}"
            )),
            // This would be quite ironic
            Err(err) => this.set_callback_error(format!("{err:#}")),
        }
    }

    unsafe extern "C" fn begin_preset(
        receiver: *const clap_preset_discovery_metadata_receiver,
        name: *const c_char,
        load_key: *const c_char,
    ) -> bool {
        check_null_ptr!(receiver, (*receiver).receiver_data);
        let this = &*((*receiver).receiver_data as *const Self);

        this.assert_same_thread("clap_preset_discovery_metadata_receiver::begin_preset()");

        let name = unsafe { util::cstr_ptr_to_mandatory_string(name) }.context(
            "'clap_preset_discovery_metadata_receiver::begin_preset()' called with an invalid \
             name parameter",
        );
        let load_key = unsafe { util::cstr_ptr_to_optional_string(load_key) }.context(
            "'clap_preset_discovery_metadata_receiver::begin_preset()' called with an invalid \
             load_key parameter",
        );
        match (name, load_key) {
            (Ok(name), Ok(load_key)) => {
                // We'll check for some errorous situations first. The `result` borrow needs to be
                // dropped before calling `maybe_write_preset()` as it will try to borrow it mutably
                {
                    let result = this.result.borrow();
                    let error_message = match (&*result, &load_key) {
                        // If there was an error then just immediately exit since nothing will change that
                        (Some(Err(_)), _) => return false,
                        (Some(Ok(PresetFile::Single(_))), None) => Some(
                            "calling 'begin_preset()' a second time for a non-container preset \
                             file with no load key is not allowed.",
                        ),
                        (Some(Ok(PresetFile::Single(_))), Some(_)) => Some(
                            "'begin_preset()' was called without a load key for the first time, \
                             and with a load key the second time. This is invalid behavior.",
                        ),
                        (Some(Ok(PresetFile::Container(_))), None) => Some(
                            "'begin_preset()' was called with a load key for the first time, and \
                             without a load key the second time. This is invalid behavior.",
                        ),
                        // If this is the first call and there are no errors then everything's fine
                        (None, _) | (Some(Ok(PresetFile::Container(_))), Some(_)) => None,
                    };

                    if let Some(error_message) = error_message {
                        this.set_callback_error(format!(
                            "Error in 'clap_preset_discovery_metadata_receiver::begin_preset()' \
                             call: {error_message}"
                        ));
                        return false;
                    }
                }

                // If this is a subsequent `begin_preset()` call for a container preset, then the
                // old preset is written to `self.result` before starting a new one.
                if load_key.is_some() {
                    this.maybe_write_preset();
                }

                // This starts the declaration of a new preset. The methods below this write to this
                // data structure, and it is finally added to `self.result` in a
                // `maybe_write_preset()` call either from here or from the drop handler.
                *this.next_load_key.borrow_mut() = load_key;
                *this.next_preset_data.borrow_mut() = Some(PartialPreset::new(name));

                true
            }
            (Err(err), _) | (_, Err(err)) => {
                this.set_callback_error(format!("{err:#}"));

                false
            }
        }
    }

    unsafe extern "C" fn add_plugin_id(
        receiver: *const clap_preset_discovery_metadata_receiver,
        plugin_id: *const clap_plugin_id,
    ) {
        check_null_ptr!(receiver, (*receiver).receiver_data, plugin_id);
        let this = &*((*receiver).receiver_data as *const Self);

        this.assert_same_thread("clap_preset_discovery_metadata_receiver::add_plugin_id()");

        let abi = unsafe { util::cstr_ptr_to_mandatory_string((*plugin_id).abi) }.context(
            "'clap_preset_discovery_metadata_receiver::add_plugin_id()' called with an invalid \
             abi field",
        );
        let id = unsafe { util::cstr_ptr_to_mandatory_string((*plugin_id).id) }.context(
            "'clap_preset_discovery_metadata_receiver::add_plugin_id()' called with an invalid id \
             field",
        );
        match (abi, id) {
            (Ok(abi), Ok(id)) => {
                let mut next_preset_data = this.next_preset_data.borrow_mut();
                let next_preset_data = match &mut *next_preset_data {
                    Some(next_preset_data) => next_preset_data,
                    None => {
                        this.set_callback_error(
                            "'clap_preset_discovery_metadata_receiver::add_plugin_id()' with no \
                             preceding 'begin_preset()' call. This is not valid.",
                        );
                        return;
                    }
                };

                if abi == "clap" {
                    next_preset_data.plugin_ids.push(PluginId {
                        abi: PluginAbi::Clap,
                        id,
                    });
                } else if abi.trim().eq_ignore_ascii_case("clap") {
                    // Let's just assume noone comes up with a painfully sarcastic 'ClAp' standard
                    this.set_callback_error(format!(
                        "'{abi}' was provided as an ABI argument to \
                         'clap_preset_discovery_metadata_receiver::add_plugin_id()'. This is \
                         probably a typo. The expected value is 'clap' in all lowercase."
                    ));
                } else {
                    next_preset_data.plugin_ids.push(PluginId {
                        abi: PluginAbi::Other(abi),
                        id,
                    });
                }
            }
            (Err(err), _) | (_, Err(err)) => this.set_callback_error(format!("{err:#}")),
        }
    }

    unsafe extern "C" fn set_soundpack_id(
        receiver: *const clap_preset_discovery_metadata_receiver,
        soundpack_id: *const c_char,
    ) {
        check_null_ptr!(receiver, (*receiver).receiver_data);
        let this = &*((*receiver).receiver_data as *const Self);

        this.assert_same_thread("clap_preset_discovery_metadata_receiver::set_soundpack_id()");

        let soundpack_id = unsafe { util::cstr_ptr_to_mandatory_string(soundpack_id) }.context(
            "'clap_preset_discovery_metadata_receiver::set_soundpack_id()' called with an invalid \
             parameter",
        );
        match soundpack_id {
            Ok(soundpack_id) => {
                let mut next_preset_data = this.next_preset_data.borrow_mut();
                let next_preset_data = match &mut *next_preset_data {
                    Some(next_preset_data) => next_preset_data,
                    None => {
                        this.set_callback_error(
                            "'clap_preset_discovery_metadata_receiver::set_soundpack_id()' with \
                             no preceding 'begin_preset()' call. This is not valid.",
                        );
                        return;
                    }
                };

                next_preset_data.soundpack_id = Some(soundpack_id);
            }
            Err(err) => this.set_callback_error(format!("{err:#}")),
        }
    }

    unsafe extern "C" fn set_flags(
        receiver: *const clap_preset_discovery_metadata_receiver,
        flags: u32,
    ) {
        check_null_ptr!(receiver, (*receiver).receiver_data);
        let this = &*((*receiver).receiver_data as *const Self);

        this.assert_same_thread("clap_preset_discovery_metadata_receiver::set_flags()");

        let mut next_preset_data = this.next_preset_data.borrow_mut();
        let next_preset_data = match &mut *next_preset_data {
            Some(next_preset_data) => next_preset_data,
            None => {
                this.set_callback_error(
                    "'clap_preset_discovery_metadata_receiver::set_flags()' with no preceding \
                     'begin_preset()' call. This is not valid.",
                );
                return;
            }
        };

        next_preset_data.flags = Some(Flags {
            is_factory_content: (flags & CLAP_PRESET_DISCOVERY_IS_FACTORY_CONTENT) != 0,
            is_user_content: (flags & CLAP_PRESET_DISCOVERY_IS_USER_CONTENT) != 0,
            is_demo_content: (flags & CLAP_PRESET_DISCOVERY_IS_DEMO_CONTENT) != 0,
            is_favorite: (flags & CLAP_PRESET_DISCOVERY_IS_FAVORITE) != 0,
        });
    }

    unsafe extern "C" fn add_creator(
        receiver: *const clap_preset_discovery_metadata_receiver,
        creator: *const c_char,
    ) {
        check_null_ptr!(receiver, (*receiver).receiver_data);
        let this = &*((*receiver).receiver_data as *const Self);

        this.assert_same_thread("clap_preset_discovery_metadata_receiver::set_creator()");

        let creator = unsafe { util::cstr_ptr_to_mandatory_string(creator) }.context(
            "'clap_preset_discovery_metadata_receiver::set_creator()' called with an invalid \
             parameter",
        );
        match creator {
            Ok(creator) => {
                let mut next_preset_data = this.next_preset_data.borrow_mut();
                let next_preset_data = match &mut *next_preset_data {
                    Some(next_preset_data) => next_preset_data,
                    None => {
                        this.set_callback_error(
                            "'clap_preset_discovery_metadata_receiver::set_creator()' with no \
                             preceding 'begin_preset()' call. This is not valid.",
                        );
                        return;
                    }
                };

                next_preset_data.creators.push(creator);
            }
            Err(err) => this.set_callback_error(format!("{err:#}")),
        }
    }

    unsafe extern "C" fn set_description(
        receiver: *const clap_preset_discovery_metadata_receiver,
        description: *const c_char,
    ) {
        check_null_ptr!(receiver, (*receiver).receiver_data);
        let this = &*((*receiver).receiver_data as *const Self);

        this.assert_same_thread("clap_preset_discovery_metadata_receiver::set_description()");

        let description = unsafe { util::cstr_ptr_to_mandatory_string(description) }.context(
            "'clap_preset_discovery_metadata_receiver::set_description()' called with an invalid \
             parameter",
        );
        match description {
            Ok(description) => {
                let mut next_preset_data = this.next_preset_data.borrow_mut();
                let next_preset_data = match &mut *next_preset_data {
                    Some(next_preset_data) => next_preset_data,
                    None => {
                        this.set_callback_error(
                            "'clap_preset_discovery_metadata_receiver::set_description()' with no \
                             preceding 'begin_preset()' call. This is not valid.",
                        );
                        return;
                    }
                };

                next_preset_data.description = Some(description);
            }
            Err(err) => this.set_callback_error(format!("{err:#}")),
        }
    }

    unsafe extern "C" fn set_timestamps(
        receiver: *const clap_preset_discovery_metadata_receiver,
        creation_time: clap_timestamp,
        modification_time: clap_timestamp,
    ) {
        check_null_ptr!(receiver, (*receiver).receiver_data);
        let this = &*((*receiver).receiver_data as *const Self);

        this.assert_same_thread("clap_preset_discovery_metadata_receiver::set_timestamps()");

        // These are parsed to `None` values if the timestamp is 0/CLAP_TIMESTAMP_UNKNOWN
        let creation_time = util::parse_timestamp(creation_time).context(
            "'clap_preset_discovery_metadata_receiver::set_timestamps()' called with an invalid \
             creation_time parameter",
        );
        let modification_time = util::parse_timestamp(modification_time).context(
            "'clap_preset_discovery_metadata_receiver::set_timestamps()' called with an invalid \
             modification_time parameter",
        );
        match (creation_time, modification_time) {
            // Calling the function like htis doesn't make any sense, so we'll point that out
            (Ok(None), Ok(None)) => this.set_callback_error(
                "'clap_preset_discovery_metadata_receiver::set_timestamps()' called with both \
                 arguments set to 'CLAP_TIMESTAMP_UNKNOWN'.",
            ),
            (Ok(creation_time), Ok(modification_time)) => {
                let mut next_preset_data = this.next_preset_data.borrow_mut();
                let next_preset_data = match &mut *next_preset_data {
                    Some(next_preset_data) => next_preset_data,
                    None => {
                        this.set_callback_error(
                            "'clap_preset_discovery_metadata_receiver::set_timestamps()' with no \
                             preceding 'begin_preset()' call. This is not valid.",
                        );
                        return;
                    }
                };

                next_preset_data.creation_time = creation_time;
                next_preset_data.modification_time = modification_time;
            }
            (Err(err), _) | (_, Err(err)) => this.set_callback_error(format!("{err:#}")),
        }
    }

    unsafe extern "C" fn add_feature(
        receiver: *const clap_preset_discovery_metadata_receiver,
        feature: *const c_char,
    ) {
        check_null_ptr!(receiver, (*receiver).receiver_data);
        let this = &*((*receiver).receiver_data as *const Self);

        this.assert_same_thread("clap_preset_discovery_metadata_receiver::add_feature()");

        let feature = unsafe { util::cstr_ptr_to_mandatory_string(feature) }.context(
            "'clap_preset_discovery_metadata_receiver::add_feature()' called with an invalid \
             parameter",
        );
        match feature {
            Ok(feature) => {
                let mut next_preset_data = this.next_preset_data.borrow_mut();
                let next_preset_data = match &mut *next_preset_data {
                    Some(next_preset_data) => next_preset_data,
                    None => {
                        this.set_callback_error(
                            "'clap_preset_discovery_metadata_receiver::add_plugin_id()' with no \
                             preceding 'begin_preset()' call. This is not valid.",
                        );
                        return;
                    }
                };

                next_preset_data.features.push(feature);
            }
            Err(err) => this.set_callback_error(format!("{err:#}")),
        }
    }

    unsafe extern "C" fn add_extra_info(
        receiver: *const clap_preset_discovery_metadata_receiver,
        key: *const c_char,
        value: *const c_char,
    ) {
        check_null_ptr!(receiver, (*receiver).receiver_data);
        let this = &*((*receiver).receiver_data as *const Self);

        this.assert_same_thread("clap_preset_discovery_metadata_receiver::add_extra_info()");

        let key = unsafe { util::cstr_ptr_to_mandatory_string(key) }.context(
            "'clap_preset_discovery_metadata_receiver::add_extra_info()' called with an invalid \
             key parameter",
        );
        let value = unsafe { util::cstr_ptr_to_mandatory_string(value) }.context(
            "'clap_preset_discovery_metadata_receiver::add_extra_info()' called with an invalid \
             value parameter",
        );
        match (key, value) {
            (Ok(key), Ok(value)) => {
                let mut next_preset_data = this.next_preset_data.borrow_mut();
                let next_preset_data = match &mut *next_preset_data {
                    Some(next_preset_data) => next_preset_data,
                    None => {
                        this.set_callback_error(
                            "'clap_preset_discovery_metadata_receiver::add_extra_info()' with no \
                             preceding 'begin_preset()' call. This is not valid.",
                        );
                        return;
                    }
                };

                next_preset_data.extra_info.insert(key, value);
            }
            (Err(err), _) | (_, Err(err)) => this.set_callback_error(format!("{err:#}")),
        }
    }
}
