//! The thread-local failure slot that backs `jp_last_error`.

use std::{
    cell::RefCell,
    ffi::CString,
    panic::{self, AssertUnwindSafe},
};

use tracing::warn;

thread_local! {
    /// The most recent failure on this thread, until `jp_last_error` takes it.
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

/// Run `body`, returning `None` when it fails or panics.
///
/// The failure message is left in the thread-local slot for `jp_last_error` to
/// collect.
/// `label` names the entry point, because a caught panic carries no location of
/// its own by the time it reaches here.
pub(crate) fn guard<T>(label: &str, body: impl FnOnce() -> Result<T, String>) -> Option<T> {
    match panic::catch_unwind(AssertUnwindSafe(body)) {
        Ok(Ok(value)) => Some(value),
        Ok(Err(message)) => {
            set(message);
            None
        }
        Err(_) => {
            set(format!("{label} panicked"));
            None
        }
    }
}

/// Take the pending failure message, leaving the slot empty.
pub(crate) fn take() -> Option<CString> {
    LAST_ERROR
        .try_with(|slot| slot.borrow_mut().take())
        .ok()
        .flatten()
}

/// Replace the pending failure message.
fn set(message: String) {
    warn!(message, "FFI call failed.");

    let message = CString::new(message).unwrap_or_else(|error| {
        // An interior NUL cannot cross a C string boundary. Truncating there
        // keeps the leading, most specific part of the message rather than
        // dropping the failure entirely.
        let bytes = error.into_vec();
        let end = bytes.iter().position(|byte| *byte == 0).unwrap_or_default();
        CString::new(&bytes[..end]).expect("no NUL before the first NUL")
    });

    // `try_with` fails only after this thread's destructors have run, at which
    // point no caller is left to read the message.
    let _err = LAST_ERROR.try_with(|slot| slot.replace(Some(message)));
}
