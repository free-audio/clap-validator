//! The indexer abstraction for a CLAP plugin's preset discovery factory. During initialization the
//! plugin fills this object with its supported locations, file types, and sound packs.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::cell::RefCell;
use std::ffi::{c_char, c_void, CStr, CString};
use std::fmt::Display;
use std::path::Path;
use std::pin::Pin;
use std::thread::ThreadId;

use clap_sys::factory::draft::preset_discovery::{
    clap_preset_discovery_filetype, clap_preset_discovery_indexer, clap_preset_discovery_location,
    clap_preset_discovery_location_kind, clap_preset_discovery_soundpack,
    CLAP_PRESET_DISCOVERY_IS_DEMO_CONTENT, CLAP_PRESET_DISCOVERY_IS_FACTORY_CONTENT,
    CLAP_PRESET_DISCOVERY_IS_FAVORITE, CLAP_PRESET_DISCOVERY_IS_USER_CONTENT,
    CLAP_PRESET_DISCOVERY_LOCATION_FILE, CLAP_PRESET_DISCOVERY_LOCATION_PLUGIN,
};
use clap_sys::version::CLAP_VERSION;
use parking_lot::Mutex;

use crate::util::{self, check_null_ptr};

#[derive(Debug)]
pub struct Indexer {
    /// The thread ID for the thread this object was created on. This object is not thread-safe, so
    /// we'll assert that all callbacks are made from this thread.
    expected_thread_id: ThreadId,
    /// A description of the first error encountered by this `Indexer`, if any. This is used to
    /// store thread safety errors and other errors as the result of callbacks. In those cases we
    /// can only handle the error after the callback has been mode.
    callback_error: RefCell<Option<String>>,

    /// The data written to this object by the plugin.
    results: RefCell<IndexerResults>,

    /// The validator's version, reported in the `clap_preset_discovery_indexer` struct.
    _clap_validator_version: CString,
    /// The vtable that's passed to the provider. The `indexer_data` field is populated with a
    /// pointer to this object.
    clap_preset_discovery_indexer: Mutex<clap_preset_discovery_indexer>,
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
    pub name: String,
    pub description: Option<String>,
    /// The file extension, doesn't contain a leading period.
    pub extension: String,
}

impl FileType {
    /// Parse a `clap_preset_discovery_fileType`, returning an error if the data is not valid.
    pub fn from_descriptor(descriptor: &clap_preset_discovery_filetype) -> Result<Self> {
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

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Flags {
    pub is_factory_content: bool,
    pub is_user_content: bool,
    pub is_demo_content: bool,
    pub is_favorite: bool,
}

impl Display for Flags {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut is_first_flag = true;

        if self.is_factory_content {
            write!(f, "factory content")?;
            is_first_flag = false;
        }
        if self.is_user_content {
            if is_first_flag {
                write!(f, "user content")?;
            } else {
                write!(f, ", user content")?;
            }
            is_first_flag = false;
        }
        if self.is_demo_content {
            if is_first_flag {
                write!(f, "demo content")?;
            } else {
                write!(f, ", demo content")?;
            }
            is_first_flag = false;
        }
        if self.is_favorite {
            if is_first_flag {
                write!(f, "favorite")?;
            } else {
                write!(f, ", favorite")?;
            }
            is_first_flag = false;
        }

        if is_first_flag {
            write!(f, "(none)")?;
        }

        Ok(())
    }
}

impl Location {
    /// Parse a `clap_preset_discovery_location`, returning an error if the data is not valid.
    pub fn from_descriptor(descriptor: &clap_preset_discovery_location) -> Result<Self> {
        Ok(Location {
            flags: Flags {
                is_factory_content: (descriptor.flags & CLAP_PRESET_DISCOVERY_IS_FACTORY_CONTENT)
                    != 0,
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
#[derive(Debug, Clone, PartialEq, PartialOrd, Eq, Ord)]
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
            LocationValue::File(path) => {
                write!(f, "CLAP_PRESET_DISCOVERY_LOCATION_FILE with path {path:?}")
            }
            LocationValue::Internal => write!(f, "CLAP_PRESET_DISCOVERY_LOCATION_PLUGIN"),
        }
    }
}

impl Serialize for LocationValue {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            LocationValue::File(path) => serializer.serialize_newtype_variant(
                "LocationValue",
                1,
                "CLAP_PRESET_DISCOVERY_LOCATION_FILE",
                // This should have alreayd been checked at this point
                path.to_str().expect("Invalid UTF-8"),
            ),
            LocationValue::Internal => serializer.serialize_newtype_variant(
                "LocationValue",
                1,
                "CLAP_PRESET_DISCOVERY_LOCATION_PLUGIN",
                // This should just resolve to a `null` value, to keep the format consistent
                &None::<()>,
            ),
        }
    }
}

impl LocationValue {
    /// Constructs an new [`LocationValue`] from a location kind and a location field. Whether this
    /// succeeds or not depends on the location kind and whether or not the location is a null
    /// pointer or not. See the preset discovery factory definition for more information.
    pub unsafe fn new(
        location_kind: clap_preset_discovery_location_kind,
        location: *const c_char,
    ) -> Result<Self> {
        match location_kind {
            CLAP_PRESET_DISCOVERY_LOCATION_FILE => {
                if location.is_null() {
                    anyhow::bail!(
                        "The location may not be a null pointer with \
                         CLAP_PRESET_DISCOVERY_LOCATION_FILE."
                    )
                }

                let path = CStr::from_ptr(location);
                let path_str = path
                    .to_str()
                    .context("Invalid UTF-8 in preset discovery location")?;
                if !path_str.starts_with('/') {
                    anyhow::bail!("'{path_str}' should be an absolute path, i.e. '/{path_str}'.");
                }

                Ok(LocationValue::File(path.to_owned()))
            }
            CLAP_PRESET_DISCOVERY_LOCATION_PLUGIN => {
                if !location.is_null() {
                    anyhow::bail!(
                        "The location must be a null pointer with \
                         CLAP_PRESET_DISCOVERY_LOCATION_PLUGIN."
                    )
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
            LocationValue::File(path) => (CLAP_PRESET_DISCOVERY_LOCATION_FILE, path.as_ptr()),
            LocationValue::Internal => (CLAP_PRESET_DISCOVERY_LOCATION_PLUGIN, std::ptr::null()),
        }
    }

    /// Get a file name (only the base name) for this location. For internal presets this returns
    /// `<plugin>`.
    pub fn file_name(&self) -> Result<String> {
        match self {
            LocationValue::File(path) => {
                let path = Path::new(path.to_str().context("Invalid UTF-8 in file path")?);

                Ok(path
                    .file_name()
                    .with_context(|| format!("{path:?} is not a valid preset path"))?
                    .to_str()
                    .unwrap()
                    .to_owned())
            }
            LocationValue::Internal => Ok(String::from("<plugin>")),
        }
    }
}

/// Data parsed from a `clap_preset_discovery_soundpack`. All of these fields except for the ID may
/// be empty.
#[derive(Debug, Clone, Serialize)]
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
    pub release_timestamp: Option<DateTime<Utc>>,
}

impl Soundpack {
    /// Parse a `clap_preset_discovery_soundpack`, returning an error if the data is not valid.
    pub fn from_descriptor(descriptor: &clap_preset_discovery_soundpack) -> Result<Self> {
        Ok(Soundpack {
            flags: Flags {
                is_factory_content: (descriptor.flags & CLAP_PRESET_DISCOVERY_IS_FACTORY_CONTENT)
                    != 0,
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
            release_timestamp: util::parse_timestamp(descriptor.release_timestamp)
                .context("Error parsing the soundpack's 'release_timestamp' field")?,
        })
    }
}

impl Drop for Indexer {
    fn drop(&mut self) {
        // The results will have been moved out of `self.results` when initializing the provider, so
        // if this does contain values then the plugin did something shady
        let results = self.results.borrow();
        if !results.file_types.is_empty()
            || !results.locations.is_empty()
            || !results.soundpacks.is_empty()
        {
            log::warn!(
                "The plugin declared more file types, locations, or soundpacks after its \
                 initialization. This is invalid behavior, but there is currently no test to \
                 check for this."
            )
        }

        if let Some(error) = self.callback_error.borrow_mut().take() {
            log::error!(
                "The validator's 'clap_preset_indexer' has detected an error during a callback \
                 that is going to be thrown away. This is a clap-validator bug. The error message \
                 is: {error}"
            )
        }
    }
}

impl Indexer {
    pub fn new() -> Pin<Box<Self>> {
        let clap_validator_version =
            CString::new(env!("CARGO_PKG_VERSION")).expect("Invalid bytes in crate version");
        let indexer = Box::pin(Self {
            expected_thread_id: std::thread::current().id(),
            callback_error: RefCell::new(None),

            results: RefCell::default(),

            clap_preset_discovery_indexer: Mutex::new(clap_preset_discovery_indexer {
                clap_version: CLAP_VERSION,
                name: b"clap-validator\0".as_ptr() as *const c_char,
                vendor: b"Robbert van der Helm\0".as_ptr() as *const c_char,
                url: b"https://github.com/free-audio/clap-validator\0".as_ptr() as *const c_char,
                version: clap_validator_version.as_ptr(),
                // This is filled with a pointer to this struct after the `Box` has been allocated
                indexer_data: std::ptr::null_mut(),
                declare_filetype: Some(Self::declare_filetype),
                declare_location: Some(Self::declare_location),
                declare_soundpack: Some(Self::declare_soundpack),
                get_extension: Some(Self::get_extension),
            }),
            _clap_validator_version: clap_validator_version,
        });

        indexer.clap_preset_discovery_indexer.lock().indexer_data =
            &*indexer as *const Self as *mut c_void;

        indexer
    }

    /// Get a `clap_preset_discovery_indexer` vtable pointer that can be passed to the
    /// `clap_preset_discovery_factory` when creating a provider.
    pub fn clap_preset_discovery_indexer_ptr(
        self: &Pin<Box<Self>>,
    ) -> *const clap_preset_discovery_indexer {
        self.clap_preset_discovery_indexer.data_ptr()
    }

    /// Get the values written to this indexer by the plugin during the
    /// `clap_preset_discovery_provider::init()` call. Returns any error that would be returned by
    /// [`callback_error_check()`][Self::callback_error_check()].
    ///
    /// This moves the values out of this object.
    pub fn results(&self) -> Result<IndexerResults> {
        self.callback_error_check()?;

        Ok(std::mem::take(&mut self.results.borrow_mut()))
    }

    /// Check whether errors happened during the plugin's callbacks. Returns the first error if
    /// there were any. Automatically called when calling [`results()`][Self::results()]. If there
    /// are errors and this function is not called before the object is destroyed, an error will be
    /// logged.
    pub fn callback_error_check(&self) -> Result<()> {
        match self.callback_error.borrow_mut().take() {
            Some(err) => anyhow::bail!(err),
            None => Ok(()),
        }
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

    /// Set the callback error field if it does not already contain a value. Earlier errors are not
    /// overwritten.
    fn set_callback_error(&self, error: impl Into<String>) {
        let mut callback_error = self.callback_error.borrow_mut();
        if callback_error.is_none() {
            *callback_error = Some(error.into());
        }
    }

    unsafe extern "C" fn declare_filetype(
        indexer: *const clap_preset_discovery_indexer,
        filetype: *const clap_preset_discovery_filetype,
    ) -> bool {
        check_null_ptr!(indexer, (*indexer).indexer_data, filetype);
        let this = &*((*indexer).indexer_data as *const Self);

        this.assert_same_thread("clap_preset_discovery_indexer::declare_filetype()");
        match FileType::from_descriptor(&*filetype) {
            Ok(file_type) => {
                this.results.borrow_mut().file_types.push(file_type);

                true
            }
            Err(err) => {
                this.set_callback_error(format!(
                    "Error in 'clap_preset_discovery_indexer::declare_filetype()' call: {err:#}"
                ));

                false
            }
        }
    }

    unsafe extern "C" fn declare_location(
        indexer: *const clap_preset_discovery_indexer,
        location: *const clap_preset_discovery_location,
    ) -> bool {
        check_null_ptr!(indexer, (*indexer).indexer_data, location);
        let this = &*((*indexer).indexer_data as *const Self);

        this.assert_same_thread("clap_preset_discovery_indexer::declare_location()");
        match Location::from_descriptor(&*location) {
            Ok(location) => {
                this.results.borrow_mut().locations.push(location);

                true
            }
            Err(err) => {
                this.set_callback_error(format!(
                    "Error in 'clap_preset_discovery_indexer::declare_location()' call: {err:#}"
                ));

                false
            }
        }
    }

    unsafe extern "C" fn declare_soundpack(
        indexer: *const clap_preset_discovery_indexer,
        soundpack: *const clap_preset_discovery_soundpack,
    ) -> bool {
        check_null_ptr!(indexer, (*indexer).indexer_data, soundpack);
        let this = &*((*indexer).indexer_data as *const Self);

        this.assert_same_thread("clap_preset_discovery_indexer::declare_soundpack()");
        match Soundpack::from_descriptor(&*soundpack) {
            Ok(soundpack) => {
                this.results.borrow_mut().soundpacks.push(soundpack);

                true
            }
            Err(err) => {
                this.set_callback_error(format!(
                    "Error in 'clap_preset_discovery_indexer::declare_soundpack()' call: {err:#}"
                ));

                false
            }
        }
    }

    unsafe extern "C" fn get_extension(
        indexer: *const clap_preset_discovery_indexer,
        extension_id: *const c_char,
    ) -> *const c_void {
        check_null_ptr!(indexer, (*indexer).indexer_data, extension_id);

        // There are currently no extensions for the preset discovery factory
        std::ptr::null()
    }
}
