use std::env;

use camino::Utf8PathBuf;
use camino_tempfile::{Utf8TempDir, tempdir};
use chrono::Duration;
use datetime_literal::datetime;
use jp_conversation::{Conversation, ConversationId};
use jp_storage::backend::FsStorageBackend;
use serial_test::serial;

use super::*;

/// Snapshot the env vars workspace opening depends on, so each test can point
/// user-local storage at a temp directory and put the process back as it was.
struct EnvGuard {
    jp: Option<String>,
    xdg: Option<String>,
}

impl EnvGuard {
    fn redirect(user_data: &Utf8PathBuf) -> Self {
        let guard = Self {
            jp: env::var("JP_USER_DATA_DIR").ok(),
            xdg: env::var("XDG_DATA_HOME").ok(),
        };

        // SAFETY: mutating the environment races with any concurrent reader in
        // the process. Every test that constructs an `EnvGuard` is marked
        // `#[serial(env_vars)]`, so no other test touches these variables
        // concurrently, and nothing under test reads them from another thread.
        unsafe {
            env::set_var("JP_USER_DATA_DIR", user_data.as_str());
            env::remove_var("XDG_DATA_HOME");
        }

        guard
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: as in `redirect` — the `#[serial(env_vars)]` tests that own
        // an `EnvGuard` are the only writers, and the guard drops while that
        // serialized test still holds the lock.
        unsafe {
            match &self.jp {
                Some(value) => env::set_var("JP_USER_DATA_DIR", value),
                None => env::remove_var("JP_USER_DATA_DIR"),
            }
            match &self.xdg {
                Some(value) => env::set_var("XDG_DATA_HOME", value),
                None => env::remove_var("XDG_DATA_HOME"),
            }
        }
    }
}

/// A workspace on disk holding one conversation with a fixed ID and title.
fn workspace_with_one_conversation() -> (Utf8TempDir, EnvGuard, Utf8PathBuf) {
    workspace_holding(&Conversation {
        title: Some("Reading list".to_owned()),
        last_activated_at: datetime!(2024-09-02 12:30:00 Z),
        ..Conversation::default()
    })
}

/// A workspace on disk holding `conversation` under a fixed ID.
fn workspace_holding(conversation: &Conversation) -> (Utf8TempDir, EnvGuard, Utf8PathBuf) {
    let tmp = tempdir().unwrap();
    let guard = EnvGuard::redirect(&tmp.path().join("user-data"));

    let root = tmp.path().join("my-workspace");
    let fs = FsStorageBackend::new(&root.join(".jp")).unwrap();
    fs.write_test_conversation(
        &ConversationId::try_from(datetime!(2024-09-01 00:00:00 Z)).unwrap(),
        conversation,
    );

    (tmp, guard, root)
}

/// The conversation ID every fixture uses, as the FFI reports it.
const CONVERSATION_ID: &str = "17251488000";

/// Write an events file for the fixture conversation, replacing the empty one.
///
/// The JSON is written verbatim so the test pins the on-disk shape the loader
/// accepts, rather than whatever the stream builder happens to emit today.
fn write_events(root: &Utf8PathBuf, events_json: &str) {
    let fs = FsStorageBackend::new(&root.join(".jp")).unwrap();
    let id = ConversationId::try_from(datetime!(2024-09-01 00:00:00 Z)).unwrap();
    let path = fs
        .conversation_events_path(&id)
        .expect("conversation exists");
    std::fs::write(path, events_json).unwrap();
}

/// Open the workspace at `root` and return the conversation's event JSON.
fn events_json(root: &Utf8PathBuf, conversation_id: &str) -> String {
    let path = CString::new(root.as_str()).unwrap();
    let id = CString::new(conversation_id).unwrap();

    // SAFETY: both `CString`s outlive the calls that borrow them. `ws` is
    // checked non-null, used only between open and close, and not touched
    // afterwards.
    unsafe {
        let ws = jp_workspace_open(path.as_ptr());
        assert!(!ws.is_null(), "open failed: {:?}", take_last_error());

        let json = take_string(jp_workspace_events(ws, id.as_ptr(), ptr::null_mut()));
        jp_workspace_close(ws);
        json
    }
}

/// Open the workspace at `root`, read the conversation's events, and return the
/// timings the call reported.
fn events_timings(root: &Utf8PathBuf, conversation_id: &str) -> String {
    let path = CString::new(root.as_str()).unwrap();
    let id = CString::new(conversation_id).unwrap();
    let mut timings: *mut c_char = ptr::null_mut();

    // SAFETY: both `CString`s outlive the calls that borrow them, and `timings`
    // is a live, writable slot. `ws` is checked non-null, used only between
    // open and close, and not touched afterwards.
    unsafe {
        let ws = jp_workspace_open(path.as_ptr());
        assert!(!ws.is_null(), "open failed: {:?}", take_last_error());

        jp_string_free(jp_workspace_events(ws, id.as_ptr(), &raw mut timings));
        jp_workspace_close(ws);
    }

    take_string(timings)
}

/// Open the workspace at `root`, read its conversations, and return the timings
/// the call reported.
fn conversations_timings(root: &Utf8PathBuf) -> String {
    let path = CString::new(root.as_str()).unwrap();
    let mut timings: *mut c_char = ptr::null_mut();

    // SAFETY: `path` outlives the call that borrows it, and `timings` is a
    // live, writable slot. `ws` is checked non-null, used only between open and
    // close, and not touched afterwards.
    unsafe {
        let ws = jp_workspace_open(path.as_ptr());
        assert!(!ws.is_null(), "open failed: {:?}", take_last_error());

        jp_string_free(jp_workspace_conversations(ws, &raw mut timings));
        jp_workspace_close(ws);
    }

    take_string(timings)
}

/// The span names in a timings payload, in the order they were measured.
fn timing_names(json: &str) -> Vec<String> {
    serde_json::from_str::<Vec<serde_json::Value>>(json)
        .expect("timings are a JSON array")
        .iter()
        .map(|span| span["name"].as_str().expect("a span has a name").to_owned())
        .collect()
}

/// Open the workspace at `root` and return its conversation JSON.
fn conversations_json(root: &Utf8PathBuf) -> String {
    let path = CString::new(root.as_str()).unwrap();

    // SAFETY: `path` is a live `CString`, so the pointer is NUL-terminated and
    // valid for the call. `ws` is checked non-null before being passed on, is
    // used only between open and close, and is not touched after closing.
    unsafe {
        let ws = jp_workspace_open(path.as_ptr());
        assert!(!ws.is_null(), "open failed: {:?}", take_last_error());

        let json = take_string(jp_workspace_conversations(ws, ptr::null_mut()));
        jp_workspace_close(ws);
        json
    }
}

/// Take the pending error as an owned string, freeing the C allocation.
fn take_last_error() -> Option<String> {
    let raw = jp_last_error();
    if raw.is_null() {
        return None;
    }

    // SAFETY: `raw` is non-null (checked above) and came from `jp_last_error`,
    // so it is a NUL-terminated string this library allocated. It is read
    // before being freed, and the pointer is not used afterwards.
    let message = unsafe {
        let message = CStr::from_ptr(raw).to_str().unwrap().to_owned();
        jp_string_free(raw);
        message
    };

    Some(message)
}

/// Read a returned string as owned, freeing the C allocation.
///
/// A null return means the call failed and left a message behind, so the
/// message is what the failure reports — without it the panic says only that
/// something went wrong.
fn take_string(raw: *mut c_char) -> String {
    assert!(
        !raw.is_null(),
        "expected a string, got null: {:?}",
        take_last_error()
    );

    // SAFETY: `raw` is non-null (checked above) and came from a library call
    // that returns an owned, NUL-terminated string. It is read before being
    // freed, and the pointer is not used afterwards.
    unsafe {
        let value = CStr::from_ptr(raw).to_str().unwrap().to_owned();
        jp_string_free(raw);
        value
    }
}

#[test]
#[serial(env_vars)]
fn conversations_returns_the_index_as_json() {
    let (_tmp, _guard, root) = workspace_with_one_conversation();

    assert_eq!(
        conversations_json(&root),
        r#"[{"id":"17251488000","title":"Reading list","last_activated_at":"2024-09-02T12:30:00Z","events_count":0}]"#
    );
}

/// Timestamps keep whatever sub-second precision the conversation was stored
/// with, so the emitted RFC 3339 string has a fractional part for any
/// conversation JP created from a wall clock.
///
/// Pinned because a decoder written against whole-second output alone (Swift's
/// `.iso8601` strategy, for one) parses the test above and then fails on every
/// real workspace.
#[test]
#[serial(env_vars)]
fn conversations_keeps_sub_second_timestamp_precision() {
    let (_tmp, _guard, root) = workspace_holding(&Conversation {
        title: Some("Reading list".to_owned()),
        last_activated_at: datetime!(2024-09-02 12:30:00 Z) + Duration::microseconds(123_456),
        ..Conversation::default()
    });

    assert_eq!(
        conversations_json(&root),
        r#"[{"id":"17251488000","title":"Reading list","last_activated_at":"2024-09-02T12:30:00.123456Z","events_count":0}]"#
    );
}

/// A pinned conversation reports when it was pinned, so a reader can group
/// pinned conversations without asking a second time.
///
/// The key is absent for an unpinned conversation, which is what keeps the
/// payload in `conversations_returns_the_index_as_json` unchanged.
#[test]
#[serial(env_vars)]
fn conversations_report_when_a_conversation_was_pinned() {
    let (_tmp, _guard, root) = workspace_holding(&Conversation {
        title: Some("Reading list".to_owned()),
        last_activated_at: datetime!(2024-09-02 12:30:00 Z),
        pinned_at: Some(datetime!(2024-09-03 08:00:00 Z)),
        ..Conversation::default()
    });

    assert_eq!(
        conversations_json(&root),
        r#"[{"id":"17251488000","title":"Reading list","last_activated_at":"2024-09-02T12:30:00Z","pinned_at":"2024-09-03T08:00:00Z","events_count":0}]"#
    );
}

/// Opening any directory inside the workspace opens the workspace, so the app
/// can hand over whatever directory the user picked.
#[test]
#[serial(env_vars)]
fn open_accepts_a_directory_inside_the_workspace() {
    let (_tmp, _guard, root) = workspace_with_one_conversation();
    let nested = root.join("src/nested");
    std::fs::create_dir_all(&nested).unwrap();

    assert!(conversations_json(&nested).contains(r#""title":"Reading list""#));
}

#[test]
#[serial(env_vars)]
fn open_reports_a_directory_that_is_not_a_workspace() {
    let tmp = tempdir().unwrap();
    let _guard = EnvGuard::redirect(&tmp.path().join("user-data"));
    // The entry point hardcodes `.jp`, so this assumes no workspace exists above
    // the temp directory. A machine where one does makes the open succeed and
    // this test fail loudly, rather than pass for the wrong reason.
    let missing = tmp.path().join("no-such-directory");

    let path = CString::new(missing.as_str()).unwrap();

    // SAFETY: `path` is a live `CString`, so the pointer is NUL-terminated and
    // valid for the call.
    let ws = unsafe { jp_workspace_open(path.as_ptr()) };

    assert!(ws.is_null());
    assert_eq!(
        take_last_error(),
        Some(format!("No workspace found at or above: {missing}"))
    );
}

/// Most recently active first, so a caller renders the list as given.
///
/// The middle conversation is half a second later than the oldest but shares
/// its whole second: ordering these as text would put it first, because `.`
/// sorts before `Z`.
#[test]
#[serial(env_vars)]
fn conversations_are_ordered_by_activity() {
    let tmp = tempdir().unwrap();
    let user_data = tmp.path().join("user-data");
    let _guard = EnvGuard::redirect(&user_data);

    let root = tmp.path().join("my-workspace");
    let fs = FsStorageBackend::new(&root.join(".jp")).unwrap();

    let activated = datetime!(2024-09-02 12:30:00 Z);
    for (day, last_activated_at) in [
        (1, activated),
        (2, activated + Duration::milliseconds(500)),
        (3, activated + Duration::hours(1)),
    ] {
        let id = ConversationId::try_from(datetime!(2024-09-01 00:00:00 Z))
            .unwrap()
            .as_deciseconds()
            + day;
        fs.write_test_conversation(
            &ConversationId::try_from_deciseconds(id).unwrap(),
            &Conversation {
                title: Some(format!("conversation {day}")),
                last_activated_at,
                ..Conversation::default()
            },
        );
    }

    let json = conversations_json(&root);
    let titles: Vec<&str> = json
        .match_indices("\"title\":\"")
        .map(|(i, m)| {
            let rest = &json[i + m.len()..];
            &rest[..rest.find('"').unwrap()]
        })
        .collect();

    assert_eq!(titles, [
        "conversation 3",
        "conversation 2",
        "conversation 1"
    ]);
}

/// The projection the app renders: turns carrying the events that have prose to
/// show, each tagged with its presentation rather than its stored event kind.
///
/// Pinned exactly, because the Swift mirror is hand-maintained and nothing else
/// links the two definitions.
#[test]
#[serial(env_vars)]
fn events_are_projected_as_turns_of_tagged_json() {
    let (_tmp, _guard, root) = workspace_with_one_conversation();
    write_events(
        &root,
        r#"[
          {"timestamp":"2024-09-01 10:00:00.0","type":"turn_start"},
          {"timestamp":"2024-09-01 10:00:01.0","type":"chat_request","content":"What does this do?","author":"Jean"},
          {"timestamp":"2024-09-01 10:00:02.0","type":"chat_response","reasoning":"thinking"},
          {"timestamp":"2024-09-01 10:00:03.0","type":"chat_response","message":"It reads conversations."}
        ]"#,
    );

    assert_eq!(
        events_json(&root, CONVERSATION_ID),
        r#"[{"index":0,"events":[{"type":"user_message","timestamp":"2024-09-01T10:00:01Z","author":"Jean","text":"What does this do?"},{"type":"assistant_message","timestamp":"2024-09-01T10:00:03Z","text":"It reads conversations."}]}]"#
    );
}

/// A stream holds more than the two kinds the reader draws, and none of the
/// rest crosses the boundary — config deltas and entries written by a build
/// this one has never heard of included.
///
/// The reader shows messages, so anything without prose is weight on the wire
/// that nothing draws.
#[test]
#[serial(env_vars)]
fn events_leave_out_the_entries_that_are_not_messages() {
    let (_tmp, _guard, root) = workspace_with_one_conversation();
    write_events(
        &root,
        r#"[
          {"timestamp":"2024-09-01 10:00:00.0","type":"config_delta","delta":{}},
          {"timestamp":"2024-09-01 10:00:01.0","type":"chat_request","content":"hi"},
          {"timestamp":"2024-09-01 10:00:02.0","type":"some_future_event"}
        ]"#,
    );

    assert_eq!(
        events_json(&root, CONVERSATION_ID),
        r#"[{"index":0,"events":[{"type":"user_message","timestamp":"2024-09-01T10:00:01Z","text":"hi"}]}]"#
    );
}

/// Events and conversation summaries report timestamps in one format, so a
/// caller needs one decoder rather than one per payload shape.
/// Storage keeps its own format; the translation happens at the boundary.
#[test]
#[serial(env_vars)]
fn event_timestamps_are_rfc3339_with_sub_second_precision_kept() {
    let (_tmp, _guard, root) = workspace_with_one_conversation();
    write_events(
        &root,
        r#"[{"timestamp":"2024-09-01 10:00:00.123456","type":"chat_request","content":"hi"}]"#,
    );

    assert_eq!(
        events_json(&root, CONVERSATION_ID),
        r#"[{"index":0,"events":[{"type":"user_message","timestamp":"2024-09-01T10:00:00.123456Z","text":"hi"}]}]"#
    );
}

/// A timestamp already stored as RFC 3339 passes through unchanged, rather than
/// being mangled by a second conversion.
#[test]
#[serial(env_vars)]
fn event_timestamps_already_rfc3339_are_left_alone() {
    let (_tmp, _guard, root) = workspace_with_one_conversation();
    write_events(
        &root,
        r#"[{"timestamp":"2024-09-01T10:00:00Z","type":"chat_request","content":"hi"}]"#,
    );

    assert_eq!(
        events_json(&root, CONVERSATION_ID),
        r#"[{"index":0,"events":[{"type":"user_message","timestamp":"2024-09-01T10:00:00Z","text":"hi"}]}]"#
    );
}

#[test]
#[serial(env_vars)]
fn events_of_an_empty_conversation_are_an_empty_array() {
    let (_tmp, _guard, root) = workspace_with_one_conversation();

    assert_eq!(events_json(&root, CONVERSATION_ID), "[]");
}

#[test]
#[serial(env_vars)]
fn events_reports_an_unparsable_conversation_id() {
    let (_tmp, _guard, root) = workspace_with_one_conversation();

    let path = CString::new(root.as_str()).unwrap();
    let id = CString::new("not-an-id").unwrap();

    // SAFETY: both `CString`s outlive the calls that borrow them, and `ws` is
    // used only between open and close.
    let json = unsafe {
        let ws = jp_workspace_open(path.as_ptr());
        assert!(!ws.is_null(), "open failed: {:?}", take_last_error());

        let json = jp_workspace_events(ws, id.as_ptr(), ptr::null_mut());
        jp_workspace_close(ws);
        json
    };

    assert!(json.is_null());
    assert!(
        take_last_error().is_some_and(|e| e.starts_with("invalid conversation ID:")),
        "expected the ID parse failure to be reported"
    );
}

#[test]
#[serial(env_vars)]
fn events_reports_a_conversation_that_is_not_in_the_workspace() {
    let (_tmp, _guard, root) = workspace_with_one_conversation();

    let path = CString::new(root.as_str()).unwrap();
    let id = CString::new("17251488999").unwrap();

    // SAFETY: both `CString`s outlive the calls that borrow them, and `ws` is
    // used only between open and close.
    let json = unsafe {
        let ws = jp_workspace_open(path.as_ptr());
        assert!(!ws.is_null(), "open failed: {:?}", take_last_error());

        let json = jp_workspace_events(ws, id.as_ptr(), ptr::null_mut());
        jp_workspace_close(ws);
        json
    };

    assert!(json.is_null());
    assert!(
        take_last_error().is_some_and(|e| e.starts_with("conversation not found:")),
        "expected the missing conversation to be reported"
    );
}

/// A read attributes its own time, so a caller can tell reaching the stream
/// from projecting it from encoding the answer, rather than being told only
/// that "the library" was slow.
///
/// `project` and not `deserialize`: the events are already typed by the time
/// this call reaches them, and nothing here parses storage.
#[test]
#[serial(env_vars)]
fn events_reports_what_the_work_cost() {
    let (_tmp, _guard, root) = workspace_with_one_conversation();
    write_events(
        &root,
        r#"[{"timestamp":"2024-09-01 10:00:00.0","type":"chat_request","content":"hi"}]"#,
    );

    assert_eq!(timing_names(&events_timings(&root, CONVERSATION_ID)), [
        "storage.read",
        "project",
        "serialize"
    ]);
}

#[test]
#[serial(env_vars)]
fn conversations_reports_what_the_work_cost() {
    let (_tmp, _guard, root) = workspace_with_one_conversation();

    assert_eq!(timing_names(&conversations_timings(&root)), [
        "index.read",
        "sort",
        "serialize"
    ]);
}

/// A slot the caller passed is written whatever happens, so it never reads back
/// whatever it declared the variable with.
/// A call that failed before doing any of the work it measures reports an empty
/// array.
#[test]
#[serial(env_vars)]
fn a_failed_read_still_writes_the_timings_slot() {
    let (_tmp, _guard, root) = workspace_with_one_conversation();

    assert_eq!(events_timings(&root, "17251488999"), "[]");
    assert!(
        take_last_error().is_some_and(|e| e.starts_with("conversation not found:")),
        "expected the missing conversation to be reported"
    );
}

/// The smallest `base_config.json` a conversation can be stored with.
///
/// A stream is only readable once its base config finalizes into a whole
/// `AppConfig`, so a conversation with an empty one fails to load and the app
/// shows "Could Not Read Conversation" instead of a transcript.
/// These two settings are the ones with no default to fall back on.
///
/// Copied verbatim in `apps/macos/UITests/WorkspaceFixture.swift`.
/// When a new setting becomes required, this constant and that one both need
/// it, and [`the_ui_test_fixture_layout_is_readable`] is what says so — in
/// seconds, with the missing field named, rather than as a UI test timing out
/// against a blank pane.
const UI_TEST_BASE_CONFIG: &str = r#"{"assistant":{"model":{"id":{"provider":"anthropic","name":"test"}}},"conversation":{"tools":{"*":{"run":"ask"}}}}"#;

/// The workspace the macOS UI tests build, read back through this boundary.
///
/// Those tests run outside the app's process and cannot call this library, so
/// they write the three storage files by hand
/// (`apps/macos/UITests/WorkspaceFixture.swift`).
/// Nothing links the two spellings, so this writes the same bytes and asserts
/// the app sees a readable conversation — a storage change that breaks the
/// Swift fixture fails here first, in seconds rather than in a minute of
/// `xcodebuild`.
#[test]
#[serial(env_vars)]
fn the_ui_test_fixture_layout_is_readable() {
    let tmp = tempdir().unwrap();
    let _guard = EnvGuard::redirect(&tmp.path().join("user-data"));

    let root = tmp.path().join("my-workspace");
    let store = root.join(".jp");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::write(
        store.join(".id"),
        "DO NOT EDIT THIS FILE! IT IS AUTO-GENERATED BY JP.\nuitst\n",
    )
    .unwrap();

    // Named by the bare ID, with no title slug: the loader finds a conversation
    // by the ID prefix, so the fixture is spared reproducing the slug rule.
    let dir = store.join("conversations").join(CONVERSATION_ID);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("metadata.json"),
        r#"{"title":"Reading list","last_activated_at":"2024-09-01 09:00:00.0"}"#,
    )
    .unwrap();
    std::fs::write(dir.join("base_config.json"), UI_TEST_BASE_CONFIG).unwrap();
    std::fs::write(
        dir.join("events.json"),
        r#"[{"timestamp":"2024-09-01 09:00:00.0","type":"chat_request","author":"Jean","content":"What is on the reading list?"},{"timestamp":"2024-09-01 09:00:01.0","type":"chat_response","message":"Three books and a paper."}]"#,
    )
    .unwrap();

    assert_eq!(
        conversations_json(&root),
        r#"[{"id":"17251488000","title":"Reading list","last_activated_at":"2024-09-01T09:00:00Z","events_count":2}]"#
    );

    assert_eq!(
        events_json(&root, CONVERSATION_ID),
        r#"[{"index":0,"events":[{"type":"user_message","timestamp":"2024-09-01T09:00:00Z","author":"Jean","text":"What is on the reading list?"},{"type":"assistant_message","timestamp":"2024-09-01T09:00:01Z","text":"Three books and a paper."}]}]"#
    );
}

#[test]
fn events_reports_a_null_handle() {
    let id = CString::new(CONVERSATION_ID).unwrap();

    // SAFETY: null is the one handle value the contract admits without an open
    // workspace behind it; the entry point checks for it before dereferencing.
    let json = unsafe { jp_workspace_events(ptr::null_mut(), id.as_ptr(), ptr::null_mut()) };

    assert!(json.is_null());
    assert_eq!(
        take_last_error(),
        Some("workspace handle is null".to_owned())
    );
}

#[test]
fn open_reports_a_null_path() {
    // SAFETY: null is the one pointer value the contract admits without a
    // string behind it; the entry point checks for it before dereferencing.
    let ws = unsafe { jp_workspace_open(ptr::null()) };

    assert!(ws.is_null());
    assert_eq!(take_last_error(), Some("path is null".to_owned()));
}

#[test]
fn conversations_reports_a_null_handle() {
    // SAFETY: null is the one handle value the contract admits without an open
    // workspace behind it; the entry point checks for it before dereferencing.
    let json = unsafe { jp_workspace_conversations(ptr::null_mut(), ptr::null_mut()) };

    assert!(json.is_null());
    assert_eq!(
        take_last_error(),
        Some("workspace handle is null".to_owned())
    );
}

/// The error slot is emptied by reading it, so a later success is not reported
/// as the earlier failure.
#[test]
fn last_error_is_taken_not_copied() {
    // SAFETY: see `open_reports_a_null_path` — a null path is handled, not
    // dereferenced.
    let ws = unsafe { jp_workspace_open(ptr::null()) };
    assert!(ws.is_null());

    assert_eq!(take_last_error(), Some("path is null".to_owned()));
    assert_eq!(take_last_error(), None);
}

/// Releasing null is a no-op, so callers need no null checks of their own.
#[test]
fn freeing_null_is_a_no_op() {
    // SAFETY: both entry points document null as accepted and return early on
    // it, which is exactly the behavior under test.
    unsafe {
        jp_workspace_close(ptr::null_mut());
        jp_string_free(ptr::null_mut());
    }
}
