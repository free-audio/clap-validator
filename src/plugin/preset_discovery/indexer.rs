//! The indexer abstraction for a CLAP plugin's preset discovery factory. During initialization the
//! plugin fills this object with its supported locations, file types, and sound packs.

use crate::cli::fail_test;
use crate::cli::tracing::{Recordable, Recorder};
use crate::plugin::preset_discovery::parse_timestamp;
use crate::plugin::util::{self, CHECK_POINTER, Proxy, Proxyable, cstr_ptr_to_string, validator_version};
use anyhow::{Context, Result};
use clap_sys::factory::preset_discovery::*;
use clap_sys::version::CLAP_VERSION;
use serde::{Deserialize, Serialize};
use std::ffi::{CString, c_char, c_void};
use std::fmt::Display;
use std::path::PathBuf;
use std::sync::Mutex;
use std::thread::ThreadId;
use time::OffsetDateTime;

#[derive(Debug)]
pub struct Indexer {
    /// The thread ID for the thread this object was created on. This object is not thread-safe, so
    /// we'll assert that all callbacks are made from this thread.
    expected_thread_id: ThreadId,

    /// The data written to this object by the plugin.
    result: Mutex<Result<IndexerResults>>,
}

/// The data written to the indexer by the plugin during the
/// `clap_preset_discovery_provider::init()` call.
#[derive(Debug, Default)]
pub struct IndexerResults {
    /// The file types added to this indexer by the plugin.
    pub file_types: Vec<FileType>,
    /// The locations added to this indexer by the plugin.
    pub locations: Vec<Location>,
    /// The soundpacks added to this indexer by the plugin.
    pub soundpacks: Vec<Soundpack>,
}

/// Data parsed from a `clap_preset_discovery_filetype`.
#[derive(Debug, Clone)]
pub struct FileType {
    #[allow(unused)]
    pub name: String,
    #[allow(unused)]
    pub description: Option<String>,
    /// The file extension, doesn't contain a leading period.
    pub extension: String,
}

impl FileType {
    /// Parse a `clap_preset_discovery_fileType`, returning an error if the data is not valid.
    pub unsafe fn from_descriptor(descriptor: *const clap_preset_discovery_filetype) -> Result<Self> {
        anyhow::ensure!(!descriptor.is_null(), "Filetype is null");
        let descriptor = unsafe { &*descriptor };

        let file_type = FileType {
            name: unsafe { util::cstr_ptr_to_mandatory_string(descriptor.name) }
                .context("Error parsing the file extension's 'name' field")?,
            description: unsafe { util::cstr_ptr_to_optional_string(descriptor.description) }
                .context("Error parsing the file extension's 'description' field")?,
            extension: unsafe { util::cstr_ptr_to_mandatory_string(descriptor.file_extension) }
                .context("Error parsing the file extension's 'file_extension' field")?,
        };

        if file_type.extension.starts_with('.') {
            anyhow::bail!(
                "File extensions may not start with periods, so '{}' is not allowed.",
                file_type.extension
            )
        }

        Ok(file_type)
    }
}

/// Data parsed from a `clap_preset_discovery_location`.
#[derive(Debug, Clone)]
pub struct Location {
    pub flags: Flags,

    pub name: String,
    /// The actual location, parsed from the location kind value and the location string.
    /// Conveniently also called location, hence `LocationValue`.
    pub value: LocationValue,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Flags {
    pub is_factory_content: bool,
    pub is_user_content: bool,
    pub is_demo_content: bool,
    pub is_favorite: bool,
}

impl Recordable for Flags {
    fn record(&self, record: &mut dyn Recorder) {
        record.record("is_factory_content", self.is_factory_content);
        record.record("is_user_content", self.is_user_content);
        record.record("is_demo_content", self.is_demo_content);
        record.record("is_favorite", self.is_favorite);
    }
}

impl Location {
    /// Parse a `clap_preset_discovery_location`, returning an error if the data is not valid.
    pub unsafe fn from_descriptor(descriptor: *const clap_preset_discovery_location) -> Result<Self> {
        anyhow::ensure!(!descriptor.is_null(), "Location is null");
        let descriptor = unsafe { &*descriptor };

        Ok(Location {
            flags: Flags {
                is_factory_content: (descriptor.flags & CLAP_PRESET_DISCOVERY_IS_FACTORY_CONTENT) != 0,
                is_user_content: (descriptor.flags & CLAP_PRESET_DISCOVERY_IS_USER_CONTENT) != 0,
                is_demo_content: (descriptor.flags & CLAP_PRESET_DISCOVERY_IS_DEMO_CONTENT) != 0,
                is_favorite: (descriptor.flags & CLAP_PRESET_DISCOVERY_IS_FAVORITE) != 0,
            },

            name: unsafe { util::cstr_ptr_to_mandatory_string(descriptor.name) }
                .context("Error parsing the location's 'name' field")?,
            // This already checks that the location's kind and location fields are valid
            value: unsafe { LocationValue::new(descriptor.kind, descriptor.location)? },
        })
    }
}

/// A location as used by the preset discovery API. These are used to refer to single files,
/// directories, and internal plugin data. Previous versions of the API used URIs instead of a
/// location kind and a location path field.
#[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Ord, Serialize, Deserialize)]
pub enum LocationValue {
    /// An absolute path to a file or a directory. The spec says nothing about trailing slashes, but
    /// the paths must at least be absolute.
    ///
    /// The path may refer to a file that does not exist. This has not yet been checked when
    /// creating the path.
    File(CString),
    /// A special location referring to data stored within this plugin's library. The 'location'
    /// string is not used here. In the C-implementation this should always be a null pointer.
    Internal,
}

impl Display for LocationValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LocationValue::File(path) => write!(f, "{}", path.to_string_lossy()),
            LocationValue::Internal => write!(f, "<plugin>"),
        }
    }
}

impl Recordable for LocationValue {
    fn record(&self, record: &mut dyn Recorder) {
        match self {
            LocationValue::File(path) => path.to_string_lossy().record(record),
            LocationValue::Internal => "<plugin>".record(record),
        }
    }
}

impl LocationValue {
    /// Constructs an new [`LocationValue`] from a location kind and a location field. Whether this
    /// succeeds or not depends on the location kind and whether or not the location is a null
    /// pointer or not. See the preset discovery factory definition for more information.
    pub unsafe fn new(location_kind: clap_preset_discovery_location_kind, location: *const c_char) -> Result<Self> {
        match location_kind {
            CLAP_PRESET_DISCOVERY_LOCATION_FILE => {
                if location.is_null() {
                    anyhow::bail!("The location may not be a null pointer with CLAP_PRESET_DISCOVERY_LOCATION_FILE.")
                }

                let path_str = unsafe { cstr_ptr_to_string(location) }
                    .context("Error parsing the location string for a file location")?
                    .unwrap_or_default();

                if !path_str.starts_with('/') {
                    anyhow::bail!("'{path_str}' should be an absolute path, i.e. '/{path_str}'.");
                }

                Ok(LocationValue::File(CString::new(path_str).unwrap()))
            }
            CLAP_PRESET_DISCOVERY_LOCATION_PLUGIN => {
                if !location.is_null() {
                    anyhow::bail!("The location must be a null pointer with CLAP_PRESET_DISCOVERY_LOCATION_PLUGIN.")
                }

                Ok(LocationValue::Internal)
            }
            n => anyhow::bail!("Unknown location kind {n}."),
        }
    }

    /// Transform this `LocationValue` back into a location kind and location pointer.
    ///
    /// # Safety
    ///
    /// The returned pointer is valid for the lifetime of this struct.
    pub fn to_raw(&self) -> (clap_preset_discovery_location_kind, *const c_char) {
        match self {
            LocationValue::File(path) => (CLAP_PRESET_DISCOVERY_LOCATION_FILE, path.as_ptr() as *const c_char),
            LocationValue::Internal => (CLAP_PRESET_DISCOVERY_LOCATION_PLUGIN, std::ptr::null()),
        }
    }

    pub fn file_path(&self) -> Option<PathBuf> {
        match self {
            LocationValue::File(path) => Some(PathBuf::from(path.to_string_lossy().to_string())),
            LocationValue::Internal => None,
        }
    }

    /// Get a file name (only the base name) for this location. For internal presets this returns
    /// `<plugin>`.
    pub fn file_name(&self) -> Result<String> {
        match self.file_path() {
            None => Ok(String::from("<plugin>")),
            Some(path) => Ok(path
                .file_name()
                .with_context(|| format!("{path:?} does not have a valid file name"))?
                .to_string_lossy()
                .to_string()),
        }
    }
}

/// Data parsed from a `clap_preset_discovery_soundpack`. All of these fields except for the ID may
/// be empty.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Soundpack {
    pub flags: Flags,

    /// An ID that the plugin can be refer to later when interacting with the metadata receiver.
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub homepage_url: Option<String>,
    pub vendor: Option<String>,
    pub image_path: Option<String>,
    pub release_timestamp: Option<OffsetDateTime>,
}

impl Soundpack {
    /// Parse a `clap_preset_discovery_soundpack`, returning an error if the data is not valid.
    pub unsafe fn from_descriptor(descriptor: *const clap_preset_discovery_soundpack) -> Result<Self> {
        anyhow::ensure!(!descriptor.is_null(), "Soundpack is null");
        let descriptor = unsafe { &*descriptor };

        Ok(Soundpack {
            flags: Flags {
                is_factory_content: (descriptor.flags & CLAP_PRESET_DISCOVERY_IS_FACTORY_CONTENT) != 0,
                is_user_content: (descriptor.flags & CLAP_PRESET_DISCOVERY_IS_USER_CONTENT) != 0,
                is_demo_content: (descriptor.flags & CLAP_PRESET_DISCOVERY_IS_DEMO_CONTENT) != 0,
                is_favorite: (descriptor.flags & CLAP_PRESET_DISCOVERY_IS_FAVORITE) != 0,
            },

            id: unsafe { util::cstr_ptr_to_mandatory_string(descriptor.id) }
                .context("Error parsing the soundpack's 'id' field")?,
            name: unsafe { util::cstr_ptr_to_mandatory_string(descriptor.name) }
                .context("Error parsing the soundpack's 'name' field")?,
            description: unsafe { util::cstr_ptr_to_optional_string(descriptor.description) }
                .context("Error parsing the soundpack's 'description' field")?,
            homepage_url: unsafe { util::cstr_ptr_to_optional_string(descriptor.homepage_url) }
                .context("Error parsing the soundpack's 'homepage_url' field")?,
            vendor: unsafe { util::cstr_ptr_to_optional_string(descriptor.vendor) }
                .context("Error parsing the soundpack's 'vendor' field")?,
            image_path: unsafe { util::cstr_ptr_to_optional_string(descriptor.image_path) }
                .context("Error parsing the soundpack's 'image_path' field")?,
            release_timestamp: parse_timestamp(descriptor.release_timestamp)
                .context("Error parsing the soundpack's 'release_timestamp' field")?,
        })
    }
}

impl Proxyable for Indexer {
    type Vtable = clap_preset_discovery_indexer;

    fn init(&self) -> Self::Vtable {
        clap_preset_discovery_indexer {
            clap_version: CLAP_VERSION,
            indexer_data: CHECK_POINTER,
            name: c"clap-validator".as_ptr(),
            vendor: c"Robbert van der Helm".as_ptr(),
            url: c"https://github.com/free-audio/clap-validator".as_ptr(),
            version: validator_version().as_ptr(),
            declare_filetype: Some(Self::declare_filetype),
            declare_location: Some(Self::declare_location),
            declare_soundpack: Some(Self::declare_soundpack),
            get_extension: Some(Self::get_extension),
        }
    }
}

impl Indexer {
    pub fn new() -> Proxy<Self> {
        Proxy::new(Self {
            expected_thread_id: std::thread::current().id(),
            result: Mutex::new(Ok(IndexerResults::default())),
        })
    }

    /// Get the values written to this indexer by the plugin during the
    /// `clap_preset_discovery_provider::init()` call. This also checks for errors that
    /// happened during the indexer callbacks.
    ///
    /// This can only be called once.
    pub fn finish(&self) -> Result<IndexerResults> {
        std::mem::replace(
            &mut *self.result.lock().unwrap(),
            Err(anyhow::anyhow!("Indexer already finished")),
        )
    }

    #[track_caller]
    fn wrap<R>(
        indexer: *const clap_preset_discovery_indexer,
        function_name: &'static str,
        f: impl FnOnce(&Self) -> Result<R>,
    ) -> Option<R> {
        let state = unsafe {
            Proxy::<Self>::from_vtable(indexer).unwrap_or_else(|e| {
                fail_test!("{}: {}", function_name, e);
            })
        };

        if Proxy::vtable(&state).indexer_data != CHECK_POINTER {
            fail_test!("{}: plugin messed with the 'indexer_data' pointer", function_name);
        }

        match f(&state) {
            Ok(result) => Some(result),
            Err(error) => {
                log::error!("{:#}", error);

                let mut guard = state.result.lock().unwrap();
                if guard.is_ok() {
                    *guard = Err(error.context(function_name.to_string()));
                }

                None
            }
        }
    }

    /// Checks that this function is called from the same thread the indexer was created on. If it
    /// is not, then an error indicating this can be retrieved using
    /// [`check_errors()`][Self::check_errors()]. Subsequent thread safety errors
    /// will not overwrite earlier ones.
    fn assert_same_thread(&self) -> Result<()> {
        let current_thread_id = std::thread::current().id();
        anyhow::ensure!(
            current_thread_id == self.expected_thread_id,
            "A 'clap_preset_indexer::*' method may only be called from the same thread the 'clap_preset_indexer' was \
             created on (thread {:?}), but it was called from thread {:?}",
            self.expected_thread_id,
            current_thread_id
        );

        Ok(())
    }

    unsafe extern "C" fn declare_filetype(
        indexer: *const clap_preset_discovery_indexer,
        filetype: *const clap_preset_discovery_filetype,
    ) -> bool {
        Self::wrap(indexer, "clap_preset_discovery_indexer::declare_filetype", |this| {
            this.assert_same_thread()?;

            let mut results = this.result.lock().unwrap();
            let Ok(results) = results.as_mut() else {
                // The indexer has already been finished, or an error has occurred
                // If the error has already occurred, we wont overwrite it
                anyhow::bail!("Attempt to add to the indexer after the 'clap_preset_discovery_factory::init' call");
            };

            results.file_types.push(unsafe { FileType::from_descriptor(filetype)? });
            Ok(true)
        })
        .unwrap_or(false)
    }

    unsafe extern "C" fn declare_location(
        indexer: *const clap_preset_discovery_indexer,
        location: *const clap_preset_discovery_location,
    ) -> bool {
        Self::wrap(indexer, "clap_preset_discovery_indexer::declare_location", |this| {
            this.assert_same_thread()?;

            let mut results = this.result.lock().unwrap();
            let Ok(results) = results.as_mut() else {
                // Same as above
                anyhow::bail!("Attempt to add to the indexer after the 'clap_preset_discovery_factory::init' call");
            };

            results.locations.push(unsafe { Location::from_descriptor(location)? });
            Ok(true)
        })
        .unwrap_or(false)
    }

    unsafe extern "C" fn declare_soundpack(
        indexer: *const clap_preset_discovery_indexer,
        soundpack: *const clap_preset_discovery_soundpack,
    ) -> bool {
        Self::wrap(indexer, "clap_preset_discovery_indexer::declare_soundpack", |this| {
            this.assert_same_thread()?;

            let mut results = this.result.lock().unwrap();
            let Ok(results) = results.as_mut() else {
                // Same as above
                anyhow::bail!("Attempt to add to the indexer after the 'clap_preset_discovery_factory::init' call");
            };

            results
                .soundpacks
                .push(unsafe { Soundpack::from_descriptor(soundpack)? });
            Ok(true)
        })
        .unwrap_or(false)
    }

    unsafe extern "C" fn get_extension(
        indexer: *const clap_preset_discovery_indexer,
        extension_id: *const c_char,
    ) -> *const c_void {
        Self::wrap(indexer, "clap_preset_discovery_indexer::get_extension", |_| {
            if extension_id.is_null() {
                anyhow::bail!("Null extension ID");
            }

            // There are currently no extensions for the preset discovery factory
            Ok(std::ptr::null())
        })
        .unwrap_or_default()
    }
}
