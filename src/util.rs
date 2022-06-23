//! Miscellaneous functions for data conversions.

use std::ffi::CStr;
use std::os::raw::c_char;

/// Convert a `*const c_char` to a `String`. Returns `None` if the pointer is a null pointer or if
/// the string is not valid UTF-8.
///
/// # Safety
///
/// `ptr` should point to a valid null terminated C-string.
pub unsafe fn cstr_ptr_to_string(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }

    CStr::from_ptr(ptr).to_str().ok().map(String::from)
}

/// Convert a null terminated `*const *const c_char` array to a `Vec<String>`. Returns `None` if the
/// first pointer is a null pointer or if any of the strings string are not valid UTF-8.
///
/// # Safety
///
/// `ptr` should point to a valid null terminated C-string array.
pub unsafe fn cstr_array_to_vec(mut ptr: *const *const c_char) -> Option<Vec<String>> {
    if ptr.is_null() {
        return None;
    }

    let mut strings = Vec::new();
    while !(*ptr).is_null() {
        strings.push(cstr_ptr_to_string(*ptr)?);
        ptr = ptr.offset(1);
    }

    Some(strings)
}
