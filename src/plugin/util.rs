//! Various utility functions for the plugin host.

use anyhow::{Context, Result};
use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::OnceLock;

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
            None => $crate::cli::fail_test!("'{}::{}' is a null pointer, but this is not allowed", $crate::plugin::util::type_name_of_ptr($obj_ptr), stringify!($function_name)),
        }
    }
}

pub(crate) use clap_call;

/// A pointer used for fields like `host_data` that can be checked for validity.
/// We do not use `host_data` etc. directly, instead we rely on the offsets within the owner struct (See [`crate::plugin::instance::PluginShared::wrap`] for more info)
pub const CHECK_POINTER: *mut c_void = 0xDEADCAFE as *mut c_void;

/// Similar to, [`std::any::type_name_of_val()`], but on stable Rust, and stripping away the pointer
/// part.
#[must_use]
#[doc(hidden)]
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

    unsafe {
        CStr::from_ptr(ptr)
            .to_str()
            .map(|str| Some(String::from(str)))
            .context("Error while parsing UTF-8")
    }
}

/// The same as [`cstr_ptr_to_string()`], but it returns an error if the string is empty.
pub unsafe fn cstr_ptr_to_mandatory_string(ptr: *const c_char) -> Result<String> {
    unsafe {
        match cstr_ptr_to_string(ptr)? {
            Some(string) if string.is_empty() => anyhow::bail!("The string is empty."),
            Some(string) => Ok(string),
            None => anyhow::bail!("The string is a null pointer."),
        }
    }
}

/// The same as [`cstr_ptr_to_string()`], but it treats empty strings as missing. Useful for parsing
/// optional fields from structs.
pub unsafe fn cstr_ptr_to_optional_string(ptr: *const c_char) -> Result<Option<String>> {
    unsafe {
        match cstr_ptr_to_string(ptr)? {
            Some(string) if string.is_empty() => Ok(None),
            x => Ok(x),
        }
    }
}

/// Convert a null terminated `*const *const c_char` array to a `Vec<String>`. Returns `None` if the
/// first pointer is a null pointer. Returns an error if any of the strings are not valid UTF-8.
///
/// # Safety
///
/// `ptr` should point to a valid null terminated C-string array.
pub unsafe fn cstr_array_to_vec(mut ptr: *const *const c_char) -> Result<Option<Vec<String>>> {
    unsafe {
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

pub fn validator_version() -> &'static CStr {
    static VERSION: OnceLock<CString> = OnceLock::new();
    VERSION
        .get_or_init(|| CString::new(env!("CARGO_PKG_VERSION")).unwrap())
        .as_c_str()
}

pub use proxy::{Proxy, Proxyable};

mod proxy {
    use anyhow::Result;
    use rustc_hash::FxHashMap;
    use std::any::{TypeId, type_name};
    use std::ops::Deref;
    use std::pin::Pin;
    use std::sync::{Arc, RwLock};

    struct TrackStatus {
        type_id: TypeId,
        type_name: &'static str,
        is_alive: bool,
    }

    static OBJECTS: RwLock<Option<FxHashMap<usize, TrackStatus>>> = RwLock::new(None);

    /// Start tracking the given object pointer.
    fn track<T: 'static>(obj: *const T) {
        let mut objects = OBJECTS.write().unwrap();
        objects.get_or_insert_default().insert(
            obj.addr(),
            TrackStatus {
                type_id: TypeId::of::<T>(),
                type_name: type_name::<T>(),
                is_alive: true,
            },
        );
    }

    /// Stop tracking the given object pointer, any subsequent use will be considered invalid.
    fn untrack<T: 'static>(obj: *const T) {
        let mut objects = OBJECTS.write().unwrap();
        match objects.as_mut().and_then(|x| x.get_mut(&obj.addr())) {
            Some(status) if TypeId::of::<T>() == status.type_id => status.is_alive = false,
            _ => unreachable!(),
        }
    }

    /// Check that the given object pointer is valid, of the correct type, and is still alive.
    fn check<T: 'static>(obj: *const T) -> Result<()> {
        if obj.is_null() {
            anyhow::bail!("null pointer to {}", type_name::<T>());
        }

        let objects = OBJECTS.read().unwrap();
        let object = objects.as_ref().and_then(|x| x.get(&obj.addr()));

        let Some(object) = object else {
            anyhow::bail!("invalid pointer to {}", type_name::<T>());
        };

        if object.type_id != TypeId::of::<T>() {
            anyhow::bail!("expected pointer to {}, got {}", type_name::<T>(), object.type_name);
        }

        if !object.is_alive {
            anyhow::bail!("{} has expired", type_name::<T>());
        }

        Ok(())
    }

    #[repr(C)]
    struct ProxyInner<T: Proxyable> {
        vtable: T::Vtable,
        data: T,
    }

    /// A type that can be proxied to the plugin through a vtable pointer.
    /// See [`Proxy`] for more information.
    pub trait Proxyable {
        type Vtable: 'static;

        fn init(&self) -> Self::Vtable;
    }

    /// An object that is accessible to the plugin through a vtable pointer.
    /// Implementors of interfaces such as [`clap_host`], [`clap_istream`], and [`clap_ostream`] should be wrapped in this type before being passed to the plugin.
    #[repr(transparent)]
    pub struct Proxy<T: Proxyable>(Pin<Arc<ProxyInner<T>>>);

    impl<T: Proxyable> Proxy<T> {
        pub fn new(vtable: T) -> Self {
            let arc = Arc::pin(ProxyInner {
                vtable: vtable.init(),
                data: vtable,
            });

            track(&arc.vtable);
            Self(arc)
        }

        pub fn vtable(this: &Self) -> &T::Vtable {
            &this.0.vtable
        }

        pub unsafe fn from_vtable(vtable: *const T::Vtable) -> Result<Self> {
            check(vtable)?;

            unsafe {
                let inner = vtable.cast::<ProxyInner<T>>();
                Arc::increment_strong_count(inner);
                Ok(Proxy(Pin::new_unchecked(Arc::from_raw(inner))))
            }
        }
    }

    impl<T: Proxyable> Clone for Proxy<T> {
        fn clone(&self) -> Self {
            Proxy(self.0.clone())
        }
    }

    impl<T: Proxyable> Deref for Proxy<T> {
        type Target = T;

        fn deref(&self) -> &Self::Target {
            &self.0.data
        }
    }

    impl<T: Proxyable> std::fmt::Debug for Proxy<T>
    where
        T: std::fmt::Debug,
    {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_tuple("Proxy").field(&self.0.data).finish()
        }
    }

    impl<T: Proxyable> Drop for ProxyInner<T> {
        fn drop(&mut self) {
            untrack(&self.vtable);
        }
    }
}
