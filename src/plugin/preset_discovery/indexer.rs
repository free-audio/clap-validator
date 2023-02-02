//! The indexer abstraction for a CLAP plugin. During initialization the plugin fills this object
//! with its supported locations, file types, and sound packs.

use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use std::cell::RefCell;
use std::ffi::{c_char, c_void, CString};
use std::path::PathBuf;
use std::pin::Pin;
use std::thread::ThreadId;

use clap_sys::factory::draft::preset_discovery::{
    clap_preset_discovery_filetype, clap_preset_discovery_indexer, clap_preset_discovery_location,
    clap_preset_discovery_soundpack, CLAP_PRESET_DISCOVERY_IS_DEMO_CONTENT,
    CLAP_PRESET_DISCOVERY_IS_FACTORY_CONTENT, CLAP_PRESET_DISCOVERY_IS_FAVORITE,
    CLAP_PRESET_DISCOVERY_IS_USER_CONTENT, CLAP_TIMESTAMP_UNKNOWN,
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
    file_types: Vec<FileType>,
    /// The locations added to this indexer by the plugin.
    locations: Vec<Location>,
    /// The soundpacks added to this indexer by the plugin.
    soundpacks: Vec<Soundpack>,
}

/// Data parsed from a `clap_preset_discovery_filetype`.
#[derive(Debug, Clone)]
pub struct FileType {
    pub name: String,
    pub description: String,
    /// The file extension, doesn't contain a leading period.
    pub extension: String,
}

impl FileType {
    /// Parse a `clap_preset_discovery_fileType`, returning an error if the data is not valid.
    pub fn from_descriptor(descriptor: &clap_preset_discovery_filetype) -> Result<Self> {
        let file_type = FileType {
            // TODO: Is the name allowed to be empty?
            name: unsafe { util::cstr_ptr_to_string(descriptor.name)? }
                .context("A file type's 'name' field was a null pointer")?,
            description: unsafe { util::cstr_ptr_to_string(descriptor.description)? }
                .context("A file type's 'description' field was a null pointer")?,
            extension: unsafe { util::cstr_ptr_to_string(descriptor.file_extension)? }
                .context("A file type's 'file_extension' field was a null pointer")?,
        };

        if file_type.extension.starts_with('.') {
            anyhow::bail!(
                "So extensions may not start with periods, so '{}' is not allowed",
                file_type.extension
            )
        }

        Ok(file_type)
    }
}

/// Data parsed from a `clap_preset_discovery_location`.
#[derive(Debug, Clone)]
pub struct Location {
    pub is_factory_content: bool,
    pub is_user_content: bool,
    pub is_demo_content: bool,
    pub is_favorite: bool,

    pub name: String,
    /// The location's URI. The exact variant determines how the location should be treated.
    pub uri: LocationUri,
}

impl Location {
    /// Parse a `clap_preset_discovery_location`, returning an error if the data is not valid.
    pub fn from_descriptor(descriptor: &clap_preset_discovery_location) -> Result<Self> {
        Ok(Location {
            is_factory_content: (descriptor.flags & CLAP_PRESET_DISCOVERY_IS_FACTORY_CONTENT) != 0,
            is_user_content: (descriptor.flags & CLAP_PRESET_DISCOVERY_IS_USER_CONTENT) != 0,
            is_demo_content: (descriptor.flags & CLAP_PRESET_DISCOVERY_IS_DEMO_CONTENT) != 0,
            is_favorite: (descriptor.flags & CLAP_PRESET_DISCOVERY_IS_FAVORITE) != 0,

            // TODO: Is the name allowed to be empty?
            name: unsafe { util::cstr_ptr_to_string(descriptor.name)? }
                .context("A location's 'name' field was a null pointer")?,
            uri: LocationUri::from_uri(
                &unsafe { util::cstr_ptr_to_string(descriptor.uri)? }
                    .context("A location's 'uri' field was a null pointer")?,
            )?,
        })
    }
}

/// A URI as used by the preset discovery API. These are used to refer to single files, directories,
/// and internal plugin data.
#[derive(Debug, Clone)]
pub enum LocationUri {
    /// A path parsed from a `file://` URI. If the URI was `file:///foo/bar`, then this will contain
    /// `/foo/bar`. The spec says nothing about trailing slashes, but the paths must at least be
    /// absolute.
    ///
    /// The file path is not yet checked for existence.
    File(PathBuf),
    /// A special URI referring to data stored within this plugin's library.
    Plugin,
}

impl LocationUri {
    /// Parse a URI string to a `LocationUri`. Returns an error if the URI was not in an expected format.
    pub fn from_uri(uri: &str) -> Result<Self> {
        if uri.is_empty() {
            anyhow::bail!("Empty URIs are not allowed.");
        }

        if let Some(path) = uri.strip_prefix("file://") {
            // Backslashes are valid characters in file paths on non-Windows platforms, so we'll
            // restrict this to just Windows. Hopefully this doesn't cause any false positives.
            #[cfg(windows)]
            if path.contains('\\') {
                anyhow::bail!("'{path}' should use forward slashes instead of backslashes.")
            }
            if !path.starts_with('/') {
                anyhow::bail!("'{uri}' should refer to an absolute path, i.e. 'file:///{path}'");
            }

            return Ok(LocationUri::File(PathBuf::from(path)));
        }

        if uri == "plugin://" {
            return Ok(LocationUri::Plugin);
        } else if uri.starts_with("plugin://") {
            // This is probably useful to have as a dedicated check
            anyhow::bail!(
                "'{uri}' is not a valid preset URI. 'plugin://' must not be followed by a path."
            )
        }

        Err(anyhow::anyhow!(
            "'{uri}' is not a supported URI, only the 'file://' and 'plugin://' schemas are \
             supported."
        ))
    }

    /// Transform this `LocationUri` back into a URI.
    pub fn to_uri(&self) -> String {
        match self {
            LocationUri::File(path) => format!(
                "fille://{}",
                path.to_str()
                    .expect("The file path contained invalid UTF-8")
            ),
            LocationUri::Plugin => String::from("plugin://"),
        }
    }
}

/// Data parsed from a `clap_preset_discovery_soundpack`. All of these fields except for the ID may
/// be empty.
#[derive(Debug, Clone)]
pub struct Soundpack {
    pub is_factory_content: bool,
    pub is_user_content: bool,
    pub is_demo_content: bool,
    pub is_favorite: bool,

    /// An ID that the plugin can be refer to later when interacting with the metadata receiver.
    pub id: String,
    pub name: String,
    pub description: String,
    pub homepage_url: String,
    pub vendor: String,
    pub image_url: String,
    pub release_timestamp: Option<DateTime<Utc>>,
}

impl Soundpack {
    /// Parse a `clap_preset_discovery_soundpack`, returning an error if the data is not valid.
    pub fn from_descriptor(descriptor: &clap_preset_discovery_soundpack) -> Result<Self> {
        let soundpack = Soundpack {
            is_factory_content: (descriptor.flags & CLAP_PRESET_DISCOVERY_IS_FACTORY_CONTENT) != 0,
            is_user_content: (descriptor.flags & CLAP_PRESET_DISCOVERY_IS_USER_CONTENT) != 0,
            is_demo_content: (descriptor.flags & CLAP_PRESET_DISCOVERY_IS_DEMO_CONTENT) != 0,
            is_favorite: (descriptor.flags & CLAP_PRESET_DISCOVERY_IS_FAVORITE) != 0,

            id: unsafe { util::cstr_ptr_to_string(descriptor.id)? }
                .context("A soundpack's 'id' field was a null pointer")?,
            // TODO: Is the name allowed to be empty?
            name: unsafe { util::cstr_ptr_to_string(descriptor.name)? }
                .context("A soundpack's 'name' field was a null pointer")?,
            description: unsafe { util::cstr_ptr_to_string(descriptor.description)? }
                .context("A soundpack's 'description' field was a null pointer")?,
            homepage_url: unsafe { util::cstr_ptr_to_string(descriptor.homepage_url)? }
                .context("A soundpack's 'homepage_url' field was a null pointer")?,
            vendor: unsafe { util::cstr_ptr_to_string(descriptor.vendor)? }
                .context("A soundpack's 'vendor' field was a null pointer")?,
            image_url: unsafe { util::cstr_ptr_to_string(descriptor.image_url)? }
                .context("A soundpack's 'image_url' field was a null pointer")?,
            release_timestamp: if descriptor.release_timestamp == CLAP_TIMESTAMP_UNKNOWN {
                None
            } else {
                Some(
                    match Utc.timestamp_millis_opt(descriptor.release_timestamp as i64) {
                        chrono::LocalResult::Single(datetime) => datetime,
                        // This shouldn't happen
                        _ => anyhow::bail!(
                            "Could not parse the timestamp from the soundpack's \
                             'release_timestamp' field"
                        ),
                    },
                )
            },
        };

        if soundpack.id.is_empty() {
            anyhow::bail!("The plugin declared a soundpack with an empty ID.")
        }

        Ok(soundpack)
    }
}

impl Drop for Indexer {
    fn drop(&mut self) {
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
    fn set_callback_error(&self, error: String) {
        let mut callback_error = self.callback_error.borrow_mut();
        if callback_error.is_none() {
            *callback_error = Some(error);
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
