//! Miscellaneous functions for data conversions.

use anyhow::{Context, Result};
use std::ffi::CStr;
use std::os::raw::c_char;
use std::path::PathBuf;

// TODO: Remove these attributes once we start implementing host interfaces

/// Early exit out of a function with the specified return value when one of the passed pointers is
/// null.
macro_rules! check_null_ptr {
    ($ret:expr, $ptr:expr $(, $ptrs:expr)* $(, )?) => {
        $crate::util::check_null_ptr_msg!("Null pointer passed to function", $ret, $ptr $(, $ptrs)*)
    };
}

/// The same as [`check_null_ptr!`], but with a custom message.
macro_rules! check_null_ptr_msg {
    ($msg:expr, $ret:expr, $ptr:expr $(, $ptrs:expr)* $(, )?) => {
        // Clippy doesn't understand it when we use a unit in our `check_null_ptr!()` maccro, even
        // if we explicitly pattern match on that unit
        #[allow(clippy::unused_unit)]
        if $ptr.is_null() $(|| $ptrs.is_null())* {
            ::log::debug!($msg);
            return $ret;
        }
    };
}

/// Call a CLAP function. This is needed because even though none of CLAP's functions are allowed to
/// be null pointers, people will still use null pointers for some of the function arguments. This
/// also happens in the official `clap-helpers`. As such, these functions are now `Option<fn(...)>`
/// optional function pointers in `clap-sys`. This macro asserts that the pointer is not null, and
/// prints a nicely formatted error message containing the struct and funciton name if it is. It
/// also emulates C's syntax for accessing fields struct through a pointer. Except that it uses `=>`
/// instead of `->`. Because that sounds like it would be hilarious.
macro_rules! clap_call {
    { $obj_ptr:expr=>$function_name:ident($($args:expr),* $(, )?) } => {
        match (*$obj_ptr).$function_name {
            Some(function_ptr) => function_ptr($($args),*),
            None => panic!("'{}::{}' is a null pointer, but this is not allowed", $crate::util::type_name_of_ptr($obj_ptr), stringify!($function_name)),
        }
    }
}

/// [`clap_call!()`], wrapped in an unsafe block.
macro_rules! unsafe_clap_call {
    { $($args:tt)* } => {
        unsafe { $crate::util::clap_call! { $($args)* } }
    }
}

pub(crate) use check_null_ptr;
pub(crate) use check_null_ptr_msg;
pub(crate) use clap_call;
pub(crate) use unsafe_clap_call;

/// Similar to, [`std::any::type_name_of_val()`], but on stable Rust, and stripping away the pointer
/// part.
#[must_use]
pub fn type_name_of_ptr<T: ?Sized>(_ptr: *const T) -> &'static str {
    std::any::type_name::<T>()
}

/// Convert a `*const c_char` to a `String`. Returns `Ok(None)` if the pointer is a null pointer or
/// if the string is not valid UTF-8. This only returns an error if the string contains invalid
/// UTF-8.
///
/// # Safety
///
/// `ptr` should point to a valid null terminated C-string.
pub unsafe fn cstr_ptr_to_string(ptr: *const c_char) -> Result<Option<String>> {
    if ptr.is_null() {
        return Ok(None);
    }

    CStr::from_ptr(ptr)
        .to_str()
        .map(|str| Some(String::from(str)))
        .context("Error while parsing UTF-8")
}

/// Convert a null terminated `*const *const c_char` array to a `Vec<String>`. Returns `None` if the
/// first pointer is a null pointer. Returns an error if any of the strings are not valid UTF-8.
///
/// # Safety
///
/// `ptr` should point to a valid null terminated C-string array.
pub unsafe fn cstr_array_to_vec(mut ptr: *const *const c_char) -> Result<Option<Vec<String>>> {
    if ptr.is_null() {
        return Ok(None);
    }

    let mut strings = Vec::new();
    while !(*ptr).is_null() {
        // We already checked for null pointers, so we can safely unwrap this
        strings.push(cstr_ptr_to_string(*ptr)?.unwrap());
        ptr = ptr.offset(1);
    }

    Ok(Some(strings))
}

/// Convert a `c_char` slice to a `String`. Returns an error if the slice did not contain a null
/// byte, or if the string is not valid UTF-8.
pub fn c_char_slice_to_string(slice: &[c_char]) -> Result<String> {
    // `from_bytes_until_nul` is still unstable, so we'll YOLO it for now by checking if the slice
    // contains a null byte and then treating it as a pointer if it does
    if !slice.contains(&0) {
        anyhow::bail!("The string buffer does not contain a null byte.")
    }

    unsafe { CStr::from_ptr(slice.as_ptr()) }
        .to_str()
        .context("Error while parsing UTF-8")
        .map(String::from)
}

/// [`std::env::temp_dir`], but taking `XDG_RUNTIME_DIR` on Linux into account.
fn temp_dir() -> PathBuf {
    #[cfg(all(unix, not(target_os = "macos")))]
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR").map(PathBuf::from) {
        if dir.is_dir() {
            return dir;
        }
    }

    std::env::temp_dir()
}

/// A temporary directory used by the validator. This is cleared when launching the validator.
pub fn validator_temp_dir() -> PathBuf {
    temp_dir().join("clap-validator")
}
