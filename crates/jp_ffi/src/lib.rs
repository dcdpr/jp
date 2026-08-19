//! A C ABI over [`jp_workspace`], for reading JP conversations from a native
//! app.
//!
//! [`jp_workspace_open`] hands back an opaque handle that the caller owns until
//! it passes the handle to [`jp_workspace_close`].
//! Reads copy their result into a freshly allocated, NUL-terminated JSON string
//! which the caller releases with [`jp_string_free`]; no lock guard, reference,
//! or borrow of workspace state crosses the boundary.
//!
//! A read also measures the phases of its own work, and reports them through an
//! optional out-parameter the caller may pass as null.
//! They ride back on the call that produced them rather than on a call of their
//! own, so timings and the work they describe cannot drift apart when two reads
//! overlap.
//!
//! Every entry point catches panics rather than letting one unwind into the
//! calling language, which would be undefined behavior.
//! A failing call returns null and leaves a message for [`jp_last_error`].

mod display;
mod error;
mod timing;

use std::{
    ffi::{CStr, CString, c_char},
    ptr,
};

use camino::Utf8Path;
use jp_conversation::ConversationId;
use jp_plugin::message::ConversationSummary;
use jp_workspace::Workspace;

use crate::{display::project_turns, error::guard, timing::Timings};

/// An open workspace, owned by the caller between [`jp_workspace_open`] and
/// [`jp_workspace_close`].
pub struct WorkspaceRef {
    workspace: Workspace,
}

/// Open the workspace containing `path` and load its conversation index.
///
/// `path` may be the workspace root or any directory inside it.
/// Returns null on failure, leaving a message for [`jp_last_error`].
///
/// Opening writes to disk: the user-local conversation store is created if
/// missing and the workspace ID is persisted, as `jp` does.
///
/// Corrupt conversations are **not** moved aside.
/// Sanitizing a store is a deliberate act that trashes data, and a reader has
/// no business doing it as a side effect of looking; a conversation whose
/// metadata will not load is simply left out of the list.
///
/// # Safety
///
/// `path` must point to a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn jp_workspace_open(path: *const c_char) -> *mut WorkspaceRef {
    guard("jp_workspace_open", || {
        // SAFETY: `path` is NUL-terminated per this function's contract. The
        // borrow does not escape: it is consumed by `Workspace::open` below,
        // which copies the path, well before this call returns to the caller
        // that owns the string.
        let path = unsafe { borrow_str(path, "path") }?;
        let mut workspace = Workspace::open(Utf8Path::new(path)).map_err(|e| e.to_string())?;

        workspace.load_conversation_index();

        Ok(WorkspaceRef { workspace })
    })
    .map_or(ptr::null_mut(), |opened| Box::into_raw(Box::new(opened)))
}

/// Return the workspace's conversations as a JSON array, most recently active
/// first.
///
/// Each element carries `id`, `title`, `last_activated_at` and `events_count`,
/// plus `pinned_at` for a pinned conversation.
/// Timestamps are RFC 3339, with a fractional-seconds part when the stored
/// value has one.
/// Returns null on failure, leaving a message for [`jp_last_error`].
/// Release the result with [`jp_string_free`].
///
/// `timings` may be null.
/// Given a slot, the call writes a JSON array of `{"name", "duration_ms"}`
/// objects naming what it spent its time on — `index.read`, `sort`,
/// `serialize` — which the caller also releases with [`jp_string_free`].
///
/// # Safety
///
/// `ws` must be a handle from [`jp_workspace_open`] that has not been closed,
/// and `timings` must be null or point to a writable `*mut c_char`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn jp_workspace_conversations(
    ws: *mut WorkspaceRef,
    timings: *mut *mut c_char,
) -> *mut c_char {
    let mut measured = Timings::default();

    let json = guard("jp_workspace_conversations", || {
        // SAFETY: `ws` is a live handle from `jp_workspace_open` per this
        // function's contract, so it points to a `WorkspaceRef` that outlives
        // the borrow. The borrow ends before this call returns, and the shared
        // reference is compatible with the caller's ownership of the handle:
        // nothing here mutates through it.
        let opened = unsafe { borrow_workspace(ws) }?;

        // Collecting first releases every read guard before the JSON is built,
        // so nothing borrowed from the workspace outlives this call.
        let mut summaries: Vec<_> = measured.measure("index.read", || {
            opened
                .workspace
                .conversations()
                .map(|(id, metadata)| ConversationSummary {
                    id: id.as_deciseconds().to_string(),
                    title: metadata.title.clone(),
                    last_activated_at: metadata.last_activated_at,
                    pinned_at: metadata.pinned_at,
                    events_count: metadata.events_count,
                })
                .collect()
        });

        // Most recently active first, which is the order a reader wants and the
        // one `jp conversation ls` shows. Ordering here rather than in each
        // caller keeps them from disagreeing, and keeps the subtlety in one
        // place: these are timestamps, and comparing them as text would put
        // `12:30:00.5Z` before `12:30:00Z` because `.` precedes `Z`.
        //
        // The ID breaks ties. It is a timestamp too, so it keeps equal-activity
        // conversations newest-first among themselves.
        measured.measure("sort", || {
            summaries.sort_by(|a, b| {
                b.last_activated_at
                    .cmp(&a.last_activated_at)
                    .then_with(|| b.id.cmp(&a.id))
            });
        });

        let json = measured.measure("serialize", || {
            serde_json::to_string(&summaries).map_err(|e| e.to_string())
        })?;

        CString::new(json).map_err(|e| format!("conversation list is not a C string: {e}"))
    });

    // SAFETY: `timings` is null or writable per this function's contract.
    unsafe { timing::publish(timings, &measured) };

    json.map_or(ptr::null_mut(), CString::into_raw)
}

/// Return a conversation's turns as a JSON array, oldest first.
///
/// `conversation_id` is the decimal decisecond timestamp that identifies the
/// conversation, as reported by [`jp_workspace_conversations`].
/// Each element carries an `index` naming where the turn sits in the
/// conversation, and an `events` array of what it has to show.
/// Each event carries a `timestamp` in RFC 3339 and a `type` tag naming how to
/// present it: `user_message` and `assistant_message`, both carrying `text`,
/// the first with an `author` where one is known.
///
/// Only those two presentations exist.
/// Tool calls, reasoning, inquiries, config changes and turn markers have no
/// prose to show and are absent, as is any turn left with nothing — so a
/// caller can draw a boundary between consecutive turns without checking
/// whether either holds anything.
///
/// The tag names the presentation rather than the stored event kind, so a
/// caller decides how to draw without keeping its own table of event kinds — a
/// table it would have to keep in step with this crate by hand.
/// Returns null on failure, leaving a message for [`jp_last_error`].
/// Release the result with [`jp_string_free`].
///
/// `timings` may be null.
/// Given a slot, the call writes a JSON array of `{"name", "duration_ms"}`
/// objects naming what it spent its time on — `storage.read`, `project`,
/// `serialize` — which the caller also releases with [`jp_string_free`].
///
/// # Safety
///
/// `ws` must be a handle from [`jp_workspace_open`] that has not been closed,
/// `conversation_id` must point to a NUL-terminated string, and `timings` must
/// be null or point to a writable `*mut c_char`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn jp_workspace_events(
    ws: *mut WorkspaceRef,
    conversation_id: *const c_char,
    timings: *mut *mut c_char,
) -> *mut c_char {
    let mut measured = Timings::default();

    let json = guard("jp_workspace_events", || {
        // SAFETY: `ws` is a live handle from `jp_workspace_open` and
        // `conversation_id` is NUL-terminated, both per this function's
        // contract. Neither borrow outlives the call.
        let (opened, id) = unsafe {
            (
                borrow_workspace(ws)?,
                borrow_str(conversation_id, "conversation_id")?,
            )
        };

        let id = ConversationId::try_from_deciseconds_str(id)
            .map_err(|e| format!("invalid conversation ID: {e}"))?;
        let handle = opened
            .workspace
            .acquire_conversation(&id)
            .map_err(|e| format!("conversation not found: {e}"))?;

        // Scoped so the read guard is released before the JSON leaves the
        // boundary: no borrow of workspace state may outlive this call.
        let json = {
            let events = measured.measure("storage.read", || {
                opened
                    .workspace
                    .events(&handle)
                    .map_err(|e| format!("failed to load events: {e}"))
            })?;

            let display = measured.measure("project", || project_turns(&events));

            measured.measure("serialize", || {
                serde_json::to_string(&display).map_err(|e| e.to_string())
            })?
        };

        CString::new(json).map_err(|e| format!("event list is not a C string: {e}"))
    });

    // SAFETY: `timings` is null or writable per this function's contract.
    unsafe { timing::publish(timings, &measured) };

    json.map_or(ptr::null_mut(), CString::into_raw)
}

/// Release a workspace handle from [`jp_workspace_open`].
///
/// Does nothing when `ws` is null.
///
/// # Safety
///
/// `ws` must be a handle from [`jp_workspace_open`], and must not be used again
/// afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn jp_workspace_close(ws: *mut WorkspaceRef) {
    if ws.is_null() {
        return;
    }

    let _closed = guard("jp_workspace_close", || {
        // SAFETY: `ws` is non-null (checked above) and came from
        // `Box::into_raw` in `jp_workspace_open`, so reclaiming it as a `Box`
        // pairs the allocation with its original allocator. The caller
        // promises not to use the handle again, so no other alias exists.
        drop(unsafe { Box::from_raw(ws) });
        Ok(())
    });
}

/// Release a string returned by this library.
///
/// Does nothing when `string` is null.
///
/// # Safety
///
/// `string` must be a pointer returned by this library, and must not be used
/// again afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn jp_string_free(string: *mut c_char) {
    if string.is_null() {
        return;
    }

    let _freed = guard("jp_string_free", || {
        // SAFETY: `string` is non-null (checked above) and came from
        // `CString::into_raw` in this library, so reclaiming it as a `CString`
        // pairs the allocation with its original allocator. The caller
        // promises not to use the pointer again, so no other alias exists.
        drop(unsafe { CString::from_raw(string) });
        Ok(())
    });
}

/// Take the calling thread's most recent failure message.
///
/// Returns null when no call has failed since the last time the message was
/// taken.
/// Release a non-null result with [`jp_string_free`].
#[unsafe(no_mangle)]
pub extern "C" fn jp_last_error() -> *mut c_char {
    // Deliberately not routed through `guard`: recording a failure writes to
    // the same slot this reads, and a failure to report a failure has nowhere
    // left to go.
    std::panic::catch_unwind(error::take)
        .ok()
        .flatten()
        .map_or(ptr::null_mut(), CString::into_raw)
}

/// Borrow a C string argument.
///
/// `name` labels the argument in the returned message.
///
/// # Safety
///
/// `ptr` must be null, or point to a NUL-terminated string that outlives the
/// returned reference.
unsafe fn borrow_str<'a>(ptr: *const c_char, name: &str) -> Result<&'a str, String> {
    if ptr.is_null() {
        return Err(format!("{name} is null"));
    }

    // SAFETY: `ptr` is non-null (checked above) and NUL-terminated per this
    // function's contract, so the string has a bounded extent. The caller also
    // guarantees it stays valid and unmodified for the returned lifetime, which
    // is what makes the unbounded `'a` sound at every call site.
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map_err(|e| format!("{name} is not valid UTF-8: {e}"))
}

/// Borrow a workspace handle.
///
/// # Safety
///
/// `ptr` must be null, or a handle from [`jp_workspace_open`] that has not been
/// closed and outlives the returned reference.
unsafe fn borrow_workspace<'a>(ptr: *mut WorkspaceRef) -> Result<&'a WorkspaceRef, String> {
    if ptr.is_null() {
        return Err("workspace handle is null".to_owned());
    }

    // SAFETY: `ptr` is non-null (checked above) and, per this function's
    // contract, an unclosed handle from `jp_workspace_open` — hence properly
    // aligned, initialized, and valid for the returned lifetime.
    Ok(unsafe { &*ptr })
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
