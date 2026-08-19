//! Swift symbol demangling through the toolchain's `libswiftDemangle`.
//!
//! Xcode ships `libswiftDemangle.dylib` inside its default toolchain.
//! It is opened on the first Swift-looking symbol and kept for the lifetime of
//! the process.
//! When no copy can be found, [`demangle`] returns `None` and the caller keeps
//! the mangled name.
//!
//! Set `SWIFT_DEMANGLE_DYLIB` to point at a specific copy; otherwise
//! `DEVELOPER_DIR`, `xcode-select -p`, and the Command Line Tools location are
//! tried in that order.
//!
//! Loading needs `dlopen`, so off unix there is nothing to load and every Swift
//! symbol keeps its mangled name.

use std::{
    ffi::{CString, c_char},
    sync::OnceLock,
};

#[cfg(unix)]
use std::{env, ffi::c_void, process::Command};

#[cfg(unix)]
use libc::{RTLD_LAZY, RTLD_LOCAL, dlopen, dlsym};

/// `swift_demangle_getDemangledName`, stable at libswiftDemangle major version
/// 1.
/// Returns the length of the demangled name even when that exceeds the output
/// buffer, or 0 when the input is not a Swift symbol.
type DemangleFn = unsafe extern "C" fn(*const c_char, *mut c_char, usize) -> usize;

/// Path of the demangler relative to a toolchain root.
#[cfg(unix)]
const RELATIVE_PATH: &str = "Toolchains/XcodeDefault.xctoolchain/usr/lib/libswiftDemangle.dylib";

/// Demangled form of `name`, or `None` when it is not a Swift symbol or no
/// demangler could be loaded.
///
/// A single leading underscore is stripped, so both the Mach-O symbol table
/// spelling (`_$s…`) and the DWARF spelling (`$s…`) are accepted.
pub fn demangle(name: &str) -> Option<String> {
    let mangled = name.strip_prefix('_').unwrap_or(name);
    if !is_mangled(mangled) {
        return None;
    }

    let demangle_fn = demangler()?;
    let input = CString::new(mangled).ok()?;

    // Generic specializations run long, but the overwhelming majority of
    // symbols fit the first buffer and never pay for the second call.
    let mut buf = vec![0u8; 512];
    let mut needed = unsafe { demangle_fn(input.as_ptr(), buf.as_mut_ptr().cast(), buf.len()) };
    if needed == 0 {
        return None;
    }
    if needed >= buf.len() {
        buf = vec![0u8; needed + 1];
        needed = unsafe { demangle_fn(input.as_ptr(), buf.as_mut_ptr().cast(), buf.len()) };
        if needed == 0 || needed >= buf.len() {
            return None;
        }
    }

    buf.truncate(needed);
    String::from_utf8(buf).ok()
}

/// Whether `name` carries a Swift mangling prefix.
///
/// Covers Swift 4.0 onwards, which is every symbol a current toolchain emits.
/// A false positive costs one rejected call into the demangler, so the check
/// errs towards being narrow.
pub fn is_mangled(name: &str) -> bool {
    let name = name.strip_prefix('_').unwrap_or(name);
    name.starts_with("$s") || name.starts_with("$S") || name.starts_with("T0")
}

/// The loaded demangler, resolved once per process.
fn demangler() -> Option<DemangleFn> {
    static DEMANGLER: OnceLock<Option<DemangleFn>> = OnceLock::new();
    *DEMANGLER.get_or_init(load)
}

#[cfg(not(unix))]
fn load() -> Option<DemangleFn> {
    None
}

#[cfg(unix)]
fn load() -> Option<DemangleFn> {
    for path in candidate_paths() {
        let Ok(path) = CString::new(path) else {
            continue;
        };

        // SAFETY: `path` is a valid NUL-terminated C string. The handle is
        // deliberately leaked; the function pointer read out of it is cached
        // for the lifetime of the process, so closing would dangle it.
        let handle = unsafe { dlopen(path.as_ptr(), RTLD_LAZY | RTLD_LOCAL) };
        if handle.is_null() {
            continue;
        }

        // SAFETY: `handle` came back non-null from `dlopen`, and the symbol
        // name is a literal NUL-terminated C string.
        let symbol = unsafe { dlsym(handle, c"swift_demangle_getDemangledName".as_ptr()) };
        if symbol.is_null() {
            continue;
        }

        // SAFETY: the symbol is libswiftDemangle's documented C entry point
        // and matches `DemangleFn`. A data pointer and a function pointer are
        // the same width on every platform this crate runs on.
        return Some(unsafe { std::mem::transmute::<*mut c_void, DemangleFn>(symbol) });
    }
    None
}

/// Places to look for the demangler, most specific first.
#[cfg(unix)]
fn candidate_paths() -> Vec<String> {
    let mut paths = Vec::new();
    if let Ok(explicit) = env::var("SWIFT_DEMANGLE_DYLIB") {
        paths.push(explicit);
    }
    if let Ok(dir) = env::var("DEVELOPER_DIR") {
        paths.push(format!("{dir}/{RELATIVE_PATH}"));
    }
    if let Some(dir) = developer_dir() {
        paths.push(format!("{dir}/{RELATIVE_PATH}"));
    }
    paths.push("/Library/Developer/CommandLineTools/usr/lib/libswiftDemangle.dylib".to_owned());
    paths
}

/// Active developer directory, per `xcode-select -p`.
#[cfg(unix)]
fn developer_dir() -> Option<String> {
    let output = Command::new("xcode-select").arg("-p").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let dir = String::from_utf8(output.stdout).ok()?;
    let dir = dir.trim();
    (!dir.is_empty()).then(|| dir.to_owned())
}

#[cfg(test)]
#[path = "swift_tests.rs"]
mod tests;
