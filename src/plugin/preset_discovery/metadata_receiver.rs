//! The metadata abstraction for a CLAP plugin's preset discovery factory. This is used when
//! querying metadata for a plugin's file. This is sort of like a state machine the plugin writes
//! one or more presets to.

use super::{Flags, LocationValue};
use crate::cli::fail_test;
use crate::plugin::preset_discovery::parse_timestamp;
use crate::plugin::util::{self, CHECK_POINTER, Proxy, Proxyable};
use anyhow::{Context, Result};
use clap_sys::factory::preset_discovery::*;
use clap_sys::timestamp::clap_timestamp;
use clap_sys::universal_plugin_id::clap_universal_plugin_id;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::c_char;
use std::fmt::Display;
use std::sync::Mutex;
use std::thread::ThreadId;
use time::OffsetDateTime;

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
pub struct MetadataReceiver {
    /// The thread ID for the thread this object was created on. This object is not thread-safe, so
    /// we'll assert that all callbacks are made from this thread.
    expected_thread_id: ThreadId,

    /// The location this metadata receiver was created for. If this is a single-file preset and a
    /// name has not been explicitly set, then the preset's name becomes the file name including the
    /// file extensions.
    location: LocationValue,
    /// The crawled location's flags. This is used as a fallback for the preset flags if the
    /// provider does not explicitly set flags for a preset.
    location_flags: Flags,

    /// See this object's docstring. If an error occurs, then the error is written here immediately.
    /// If the object is dropped and all presets have been written to `pending_presets` without any
    /// errors occurring, then this will contain a [`PresetFile`] describing the preset(s) added by
    /// the plugin.
    result: Mutex<Result<Option<PresetFile>>>,

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
}

/// One or more presets declared by the plugin through a preset provider metadata receiver.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub name: PresetName,
    pub plugin_ids: Vec<PluginId>,
    pub soundpack_id: Option<String>,
    /// These may remain unset, in which case the host should inherit them from the location
    pub flags: Option<Flags>,
    pub creators: Vec<String>,
    pub description: Option<String>,
    pub creation_time: Option<OffsetDateTime>,
    pub modification_time: Option<OffsetDateTime>,
    pub features: Vec<String>,
    pub extra_info: BTreeMap<String, String>,
}

impl PartialPreset {
    pub fn new(name: PresetName) -> Self {
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
            anyhow::bail!("The preset '{}' was defined without setting a plugin ID.", self.name);
        }

        Ok(Preset {
            name: self.name,
            plugin_ids: self.plugin_ids,
            soundpack_id: self.soundpack_id,
            flags: match self.flags {
                Some(flags) => PresetFlags {
                    flags,
                    is_inherited: false,
                },
                None => PresetFlags {
                    flags: *location_flags,
                    is_inherited: true,
                },
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

/// The docs specify that you are not allowed to specify a preset name unless the preset is part of
/// a container file.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "type", content = "value")]
pub enum PresetName {
    Explicit(String),
    Filename(String),
}

impl Display for PresetName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PresetName::Explicit(name) => write!(f, "{name}"),
            PresetName::Filename(name) => write!(f, "{name} (derived from filename)"),
        }
    }
}

/// The plugin ABI the preset was defined for. Most plugins will define only presets for CLAP
/// plugins.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct PluginId {
    pub abi: PluginAbi,
    pub id: String,
}

/// The plugin ABI the preset was defined for. Most plugins will define only presets for CLAP
/// plugins.
#[derive(Debug, Clone, PartialEq)]
pub enum PluginAbi {
    Clap,
    Other(String),
}

impl Serialize for PluginAbi {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            PluginAbi::Clap => serializer.serialize_str("clap"),
            PluginAbi::Other(abi) => serializer.serialize_str(abi),
        }
    }
}

impl<'de> Deserialize<'de> for PluginAbi {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s: String = Deserialize::deserialize(deserializer)?;
        if s == "clap" {
            Ok(PluginAbi::Clap)
        } else {
            Ok(PluginAbi::Other(s))
        }
    }
}

/// A preset as declared by the plugin. Constructed from a [`PartialPreset`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Preset {
    pub name: PresetName,
    pub plugin_ids: Vec<PluginId>,
    pub soundpack_id: Option<String>,
    pub flags: PresetFlags,
    pub creators: Vec<String>,
    pub description: Option<String>,

    pub creation_time: Option<OffsetDateTime>,
    pub modification_time: Option<OffsetDateTime>,

    pub features: Vec<String>,
    pub extra_info: BTreeMap<String, String>,
}

/// The flags applying to a preset. These are either explicitly set for the preset or inherited from
/// the location.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct PresetFlags {
    #[serde(flatten)]
    pub flags: Flags,
    pub is_inherited: bool,
}

impl Proxyable for MetadataReceiver {
    type Vtable = clap_preset_discovery_metadata_receiver;

    fn init(&self) -> Self::Vtable {
        clap_preset_discovery_metadata_receiver {
            receiver_data: CHECK_POINTER,
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
        }
    }
}

impl MetadataReceiver {
    /// Create a new metadata receiver.
    pub fn new(location: LocationValue, location_flags: Flags) -> Proxy<Self> {
        Proxy::new(Self {
            expected_thread_id: std::thread::current().id(),

            location,
            location_flags,
            result: Mutex::new(Ok(None)),
            next_preset_data: RefCell::new(None),
            next_load_key: RefCell::new(None),
        })
    }

    /// Finish the preset declaration process and return the result. This finishes any pending
    /// presets and returns the [`PresetFile`].
    pub fn finish(&self) -> Result<Option<PresetFile>> {
        self.flush_preset()?;
        std::mem::replace(&mut *self.result.lock().unwrap(), Ok(None))
    }

    /// Checks that this function is called from the same thread the indexer was created on.
    fn assert_same_thread(&self) -> Result<()> {
        let current_thread_id = std::thread::current().id();
        anyhow::ensure!(
            current_thread_id == self.expected_thread_id,
            "'clap_preset_discovery_metadata_receiver' methods may only be called from the same thread the \
             'clap_preset_indexer' was created on (thread {:?}), but it was called from thread {:?}",
            self.expected_thread_id,
            current_thread_id
        );
        Ok(())
    }

    #[track_caller]
    fn wrap<R>(
        receiver: *const clap_preset_discovery_metadata_receiver,
        function_name: &str,
        f: impl FnOnce(&Self) -> Result<R>,
    ) -> Option<R> {
        let state = unsafe {
            Proxy::<Self>::from_vtable(receiver).unwrap_or_else(|e| {
                fail_test!("{}: {}", function_name, e);
            })
        };

        if Proxy::vtable(&state).receiver_data != CHECK_POINTER {
            fail_test!("{}: plugin messed with the 'receiver_data' pointer", function_name);
        }

        match f(&state) {
            Ok(result) => Some(result),
            Err(error) => {
                let mut guard = state.result.lock().unwrap();
                if guard.is_ok() {
                    *guard = Err(error.context(function_name.to_string()));
                }

                None
            }
        }
    }

    /// If `self.next_preset_data` is non-empty, then transform the data into a [`Preset`] and write
    /// it to a [`PresetFile`] stored in `self.result`. This is a single file or a container file
    /// depending on whether a load key was passed to the `begin_preset()` function. If multiple
    /// presets are written for a single-file preset, then an error will be written to the result.
    /// If an error was previously written, then it will not be overwritten.
    fn flush_preset(&self) -> Result<()> {
        let Some(partial_preset) = self.next_preset_data.borrow_mut().take() else {
            return Ok(()); // No preset to flush
        };

        let mut result = self.result.lock().unwrap();
        let Ok(result) = result.as_mut() else {
            return Ok(()); // An error was already produced, no one cares
        };

        let preset = partial_preset.finalize(&self.location_flags)?;
        let load_key = self.next_load_key.borrow_mut().take();

        match (result, load_key) {
            (result @ None, None) => *result = Some(PresetFile::Single(preset)),
            (result @ None, Some(load_key)) => {
                let mut presets = BTreeMap::new();
                presets.insert(load_key, preset);
                *result = Some(PresetFile::Container(presets));
            }
            (Some(PresetFile::Container(presets)), Some(load_key)) => {
                presets.insert(load_key, preset);
            }
            _ => unreachable!(),
        }

        Ok(())
    }

    unsafe extern "C" fn on_error(
        receiver: *const clap_preset_discovery_metadata_receiver,
        os_error: i32,
        error_message: *const c_char,
    ) {
        Self::wrap(
            receiver,
            "clap_preset_discovery_metadata_receiver::on_error",
            |this| -> Result<()> {
                this.assert_same_thread()?;

                let error_message =
                    unsafe { util::cstr_ptr_to_mandatory_string(error_message) }.context("Error message is invalid")?;

                anyhow::bail!(
                    "Load error occurred: OS error code {os_error} with the following error message: {error_message}"
                );
            },
        );
    }

    unsafe extern "C" fn begin_preset(
        receiver: *const clap_preset_discovery_metadata_receiver,
        name: *const c_char,
        load_key: *const c_char,
    ) -> bool {
        Self::wrap(
            receiver,
            "clap_preset_discovery_metadata_receiver::begin_preset",
            |this| {
                this.assert_same_thread()?;

                let name = unsafe { util::cstr_ptr_to_optional_string(name) }.context("Name argument is invalid")?;
                let load_key =
                    unsafe { util::cstr_ptr_to_optional_string(load_key) }.context("Load key argument is invalid")?;

                let result = this.result.lock().unwrap();
                match (&*result, &load_key) {
                    (Err(_), _) => return Ok(false),
                    (Ok(Some(PresetFile::Single(_))), _) => anyhow::bail!(
                        "Calling 'begin_preset()' a second time for a non-container (no load key) preset file is not \
                         allowed"
                    ),
                    (Ok(Some(PresetFile::Container(_))), None) => anyhow::bail!(
                        "'begin_preset()' was called with a load key for the first time, and without a load key the \
                         second time. This is invalid behavior"
                    ),
                    _ => {}
                }

                // Container presets have a load key, single-preset files don't have a load key. The
                // name field is mandatory for container presets, and optional for non-container
                // presets. If it's not specified we'll use the file name instead.
                let preset_name = match (name, &load_key) {
                    (Some(name), _) => PresetName::Explicit(name),
                    (None, Some(_)) => anyhow::bail!("Container presets must specify a preset name"),
                    (None, None) => PresetName::Filename(
                        this.location
                            .file_name()
                            .with_context(|| format!("Could not derive a file name from {}", this.location))?,
                    ),
                };

                // If this is a subsequent `begin_preset()` call for a container preset, then the
                // old preset is written to `self.result` before starting a new one.
                if load_key.is_some() {
                    this.flush_preset()?;
                }

                // This starts the declaration of a new preset. The methods below this write to this
                // data structure, and it is finally added to `self.result` in a
                // `maybe_write_preset()` call either from here or from the drop handler.
                *this.next_load_key.borrow_mut() = load_key;
                *this.next_preset_data.borrow_mut() = Some(PartialPreset::new(preset_name));

                Ok(true)
            },
        )
        .unwrap_or(false)
    }

    unsafe extern "C" fn add_plugin_id(
        receiver: *const clap_preset_discovery_metadata_receiver,
        plugin_id: *const clap_universal_plugin_id,
    ) {
        Self::wrap(
            receiver,
            "clap_preset_discovery_metadata_receiver::add_plugin_id",
            |this| {
                this.assert_same_thread()?;

                let abi = unsafe { util::cstr_ptr_to_mandatory_string((*plugin_id).abi) }
                    .context("'plugin_id.abi' is invalid")?;
                let id = unsafe { util::cstr_ptr_to_mandatory_string((*plugin_id).id) }
                    .context("'plugin_id.id' is invalid")?;

                let mut next_preset_data = this.next_preset_data.borrow_mut();
                let next_preset_data = match &mut *next_preset_data {
                    Some(next_preset_data) => next_preset_data,
                    None => anyhow::bail!("No preceding 'begin_preset()' call. This is not valid."),
                };

                if abi == "clap" {
                    next_preset_data.plugin_ids.push(PluginId {
                        abi: PluginAbi::Clap,
                        id,
                    });
                } else if abi.trim().eq_ignore_ascii_case("clap") {
                    // Let's just assume noone comes up with a painfully sarcastic 'ClAp' standard
                    anyhow::bail!(
                        "'{abi}' was provided as an ABI argument. This is probably a typo. The expected value is \
                         'clap' in all lowercase."
                    );
                } else {
                    next_preset_data.plugin_ids.push(PluginId {
                        abi: PluginAbi::Other(abi),
                        id,
                    });
                }

                Ok(())
            },
        );
    }

    unsafe extern "C" fn set_soundpack_id(
        receiver: *const clap_preset_discovery_metadata_receiver,
        soundpack_id: *const c_char,
    ) {
        Self::wrap(
            receiver,
            "clap_preset_discovery_metadata_receiver::set_soundpack_id",
            |this| {
                this.assert_same_thread()?;

                let soundpack_id =
                    unsafe { util::cstr_ptr_to_mandatory_string(soundpack_id) }.context("Soundpack ID is invalid")?;

                let mut next_preset_data = this.next_preset_data.borrow_mut();
                let next_preset_data = match &mut *next_preset_data {
                    Some(next_preset_data) => next_preset_data,
                    None => anyhow::bail!("No preceding 'begin_preset()' call. This is not valid."),
                };

                next_preset_data.soundpack_id = Some(soundpack_id);
                Ok(())
            },
        );
    }

    unsafe extern "C" fn set_flags(receiver: *const clap_preset_discovery_metadata_receiver, flags: u32) {
        Self::wrap(receiver, "clap_preset_discovery_metadata_receiver::set_flags", |this| {
            this.assert_same_thread()?;

            let mut next_preset_data = this.next_preset_data.borrow_mut();
            let next_preset_data = match &mut *next_preset_data {
                Some(next_preset_data) => next_preset_data,
                None => anyhow::bail!("No preceding 'begin_preset()' call. This is not valid."),
            };

            next_preset_data.flags = Some(Flags {
                is_factory_content: (flags & CLAP_PRESET_DISCOVERY_IS_FACTORY_CONTENT) != 0,
                is_user_content: (flags & CLAP_PRESET_DISCOVERY_IS_USER_CONTENT) != 0,
                is_demo_content: (flags & CLAP_PRESET_DISCOVERY_IS_DEMO_CONTENT) != 0,
                is_favorite: (flags & CLAP_PRESET_DISCOVERY_IS_FAVORITE) != 0,
            });

            Ok(())
        });
    }

    unsafe extern "C" fn add_creator(receiver: *const clap_preset_discovery_metadata_receiver, creator: *const c_char) {
        Self::wrap(
            receiver,
            "clap_preset_discovery_metadata_receiver::add_creator",
            |this| {
                this.assert_same_thread()?;

                let creator = unsafe { util::cstr_ptr_to_mandatory_string(creator) }.context("Creator is invalid")?;

                let mut next_preset_data = this.next_preset_data.borrow_mut();
                let next_preset_data = match &mut *next_preset_data {
                    Some(next_preset_data) => next_preset_data,
                    None => anyhow::bail!("No preceding 'begin_preset()' call. This is not valid."),
                };

                next_preset_data.creators.push(creator);
                Ok(())
            },
        );
    }

    unsafe extern "C" fn set_description(
        receiver: *const clap_preset_discovery_metadata_receiver,
        description: *const c_char,
    ) {
        Self::wrap(
            receiver,
            "clap_preset_discovery_metadata_receiver::set_description",
            |this| {
                this.assert_same_thread()?;

                let description =
                    unsafe { util::cstr_ptr_to_mandatory_string(description) }.context("Description is invalid")?;

                let mut next_preset_data = this.next_preset_data.borrow_mut();
                let next_preset_data = match &mut *next_preset_data {
                    Some(next_preset_data) => next_preset_data,
                    None => anyhow::bail!("No preceding 'begin_preset()' call. This is not valid."),
                };

                next_preset_data.description = Some(description);
                Ok(())
            },
        );
    }

    unsafe extern "C" fn set_timestamps(
        receiver: *const clap_preset_discovery_metadata_receiver,
        creation_time: clap_timestamp,
        modification_time: clap_timestamp,
    ) {
        Self::wrap(
            receiver,
            "clap_preset_discovery_metadata_receiver::set_timestamps",
            |this| {
                this.assert_same_thread()?;

                // These are parsed to `None` values if the timestamp is 0/CLAP_TIMESTAMP_UNKNOWN
                let creation_time = parse_timestamp(creation_time).context("Creation time is invalid")?;
                let modification_time = parse_timestamp(modification_time).context("Modification time is invalid")?;

                anyhow::ensure!(
                    creation_time.is_some() || modification_time.is_some(),
                    "Both arguments are set to 'CLAP_TIMESTAMP_UNKNOWN'"
                );

                let mut next_preset_data = this.next_preset_data.borrow_mut();
                let next_preset_data = match &mut *next_preset_data {
                    Some(next_preset_data) => next_preset_data,
                    None => anyhow::bail!("No preceding 'begin_preset()' call. This is not valid."),
                };

                next_preset_data.creation_time = creation_time;
                next_preset_data.modification_time = modification_time;

                Ok(())
            },
        );
    }

    unsafe extern "C" fn add_feature(receiver: *const clap_preset_discovery_metadata_receiver, feature: *const c_char) {
        Self::wrap(
            receiver,
            "clap_preset_discovery_metadata_receiver::add_feature",
            |this| {
                this.assert_same_thread()?;
                let feature = unsafe { util::cstr_ptr_to_mandatory_string(feature) }.context("Feature is invalid")?;

                let mut next_preset_data = this.next_preset_data.borrow_mut();
                let next_preset_data = match &mut *next_preset_data {
                    Some(next_preset_data) => next_preset_data,
                    None => anyhow::bail!("No preceding 'begin_preset()' call. This is not valid."),
                };

                next_preset_data.features.push(feature);
                Ok(())
            },
        );
    }

    unsafe extern "C" fn add_extra_info(
        receiver: *const clap_preset_discovery_metadata_receiver,
        key: *const c_char,
        value: *const c_char,
    ) {
        Self::wrap(
            receiver,
            "clap_preset_discovery_metadata_receiver::add_extra_info",
            |this| {
                this.assert_same_thread()?;

                let key = unsafe { util::cstr_ptr_to_mandatory_string(key) }.context("Key is invalid")?;
                let value = unsafe { util::cstr_ptr_to_mandatory_string(value) }.context("Value is invalid")?;

                let mut next_preset_data = this.next_preset_data.borrow_mut();
                let next_preset_data = match &mut *next_preset_data {
                    Some(next_preset_data) => next_preset_data,
                    None => anyhow::bail!("No preceding 'begin_preset()' call. This is not valid."),
                };

                next_preset_data.extra_info.insert(key, value);
                Ok(())
            },
        );
    }
}
