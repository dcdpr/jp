use camino_tempfile::{Utf8TempDir, tempdir};
#[cfg(unix)]
use chrono::Utc;
use jp_conversation::{Conversation, ConversationId, event::ChatResponse};
#[cfg(unix)]
use jp_plugin::registry::ApprovedPlugin;
use jp_plugin::{
    message::{ExitMessage, InterruptRequest, OptionalId, ReadEventsRequest, ReadyMessage},
    registry::{PluginKind, RegistryPlugin},
};
use jp_storage::backend::{FsStorageBackend, PersistBackend as _};
use relative_path::RelativePathBuf;
use serde_json::json;
use serial_test::serial;

use super::*;
#[cfg(unix)]
use crate::Globals;
use crate::{editor::CUT_MARKER, env_testing::EnvVarGuard};

/// A workspace no request in these tests reaches into, so it needs no storage.
fn bare_workspace() -> Workspace {
    Workspace::in_memory("/tmp/jp-test-plugin")
}

/// How a conversation is spelled on the wire, matching `list_conversations`.
fn wire_id(id: ConversationId) -> String {
    id.to_string()
}

/// A fixed conversation id, distinct per `secs`.
fn conversation_id(secs: u64) -> ConversationId {
    ConversationId::try_from(
        chrono::DateTime::<chrono::Utc>::UNIX_EPOCH + std::time::Duration::from_secs(secs),
    )
    .unwrap()
}

/// The turn is told to stop whether or not anything is listening to logs.
///
/// A run whose `--log-file` names a directory that does not exist installs no
/// tracing subscriber at all, and a `tracing` field expression does not run
/// when its callsite is disabled.
/// These tests install no subscriber either, so a delivery written inside the
/// macro is never made.
#[test]
fn an_interrupt_is_issued_without_a_tracing_subscriber() {
    let turns = RunningTurns::default();

    let id = conversation_id(1_700_000_000);
    let mut interrupted = turns.register(id);

    // Fire-and-forget, so nothing waits on an answer: queueing it is all the
    // request does.
    assert!(handle_interrupt(InterruptRequest::stop(wire_id(id)), &turns).is_none());

    assert_eq!(
        interrupted.try_next(),
        Some(InterruptAction::Stop),
        "the turn was never told to stop"
    );
}

/// An interrupt naming something that is not a conversation id is the plugin's
/// bug, and must not be mistaken for a turn that already finished.
#[tokio::test]
async fn an_unparseable_interrupt_reaches_no_handler() {
    let turns = RunningTurns::default();

    let mut interrupted = turns.register(conversation_id(1_700_000_000));

    let answer = handle_interrupt(
        InterruptRequest::stop("not-an-id".to_owned()).with_id("req-1".to_owned()),
        &turns,
    )
    .expect("a request with an id is answered")
    .await;

    assert_eq!(
        interrupted.try_next(),
        None,
        "a malformed id must not stop an unrelated turn"
    );
    assert!(
        matches!(answer, HostToPlugin::Error(ErrorResponse { ref message, .. }) if !message.contains("no turn is running")),
        "reported as the plugin's mistake, not as a finished turn: {answer:?}"
    );
}

/// A workspace holding one conversation already on disk.
///
/// The temp dir comes back so the caller keeps it alive; dropping it takes the
/// storage with it.
fn workspace_with_conversation() -> (Workspace, ConversationId, Utf8TempDir) {
    let (ws, id, _fs, tmp) = workspace_with_drafts();
    (ws, id, tmp)
}

/// The same, with user-local storage configured so drafts have somewhere to go.
fn workspace_with_drafts() -> (
    Workspace,
    ConversationId,
    Arc<FsStorageBackend>,
    Utf8TempDir,
) {
    let tmp = tempdir().unwrap();
    let fs = Arc::new(
        FsStorageBackend::new(&tmp.path().join(".jp"))
            .unwrap()
            .with_user_storage(&tmp.path().join("user"), None, "test-workspace")
            .unwrap(),
    );

    let id = ConversationId::try_from(
        chrono::DateTime::<chrono::Utc>::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000),
    )
    .unwrap();
    fs.write_test_conversation(&id, &Conversation::default());

    let mut workspace = Workspace::in_memory(tmp.path()).with_backend(fs.clone());
    workspace.load_conversation_index();

    (workspace, id, fs, tmp)
}

/// Unwrap a draft response, or say what came back instead.
fn draft(response: HostToPlugin) -> jp_plugin::message::DraftResponse {
    match response {
        HostToPlugin::Draft(draft) => draft,
        other => panic!("expected a draft response, got {other:?}"),
    }
}

/// Unwrap an events response, or say what came back instead.
fn events(response: HostToPlugin) -> jp_plugin::message::EventsResponse {
    match response {
        HostToPlugin::Events(events) => events,
        other => panic!("expected an events response, got {other:?}"),
    }
}

/// Every entry a plugin reads carries the `event_id` its stored form has.
///
/// This is the one user-visible consequence of stable event IDs: a plugin can
/// name an entry it read and have that name still mean the same entry later.
/// Asserted for every entry rather than the first, because only conversation
/// events reach the iteration views — a compaction is addressable here and
/// nowhere else.
#[test]
fn read_events_gives_a_plugin_each_entrys_id() {
    let (ws, id, _tmp) = workspace_with_conversation();
    let handle = ws.acquire_conversation(&id).unwrap();
    let stored_ids = ws.test_lock(handle).as_mut().update_events(|stream| {
        stream.start_turn("question");
        stream.add_compaction(jp_conversation::Compaction::new(0, 0));
        stream
            .to_parts()
            .unwrap()
            .1
            .iter()
            .map(|event| event["event_id"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    });

    let read = events(handle_read_events(&ws, &wire_id(id), None));

    assert_eq!(read.conversation, wire_id(id));
    let read_ids: Vec<_> = read
        .data
        .iter()
        .map(|event| event["event_id"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(read_ids, stored_ids);
    assert!(read_ids.iter().all(|id| !id.is_empty()));
}

/// Decoding content for the plugin must not disturb the entry's identity.
#[test]
fn read_events_decodes_content_without_touching_the_id() {
    let (ws, id, _tmp) = workspace_with_conversation();
    let handle = ws.acquire_conversation(&id).unwrap();
    ws.test_lock(handle)
        .as_mut()
        .update_events(|stream| stream.start_turn("a question"));

    let read = events(handle_read_events(&ws, &wire_id(id), None));

    let request = read
        .data
        .iter()
        .find(|event| event["type"] == "chat_request")
        .expect("the turn's chat request");
    assert_eq!(request["content"], "a question");
    assert!(
        request["event_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
    );
}

/// A conversation with no draft reads back empty rather than as an error: most
/// conversations never have one.
#[test]
fn an_absent_draft_reads_as_empty() {
    let (ws, id, fs, _tmp) = workspace_with_drafts();

    let read = draft(handle_read_draft(Some(&fs), &ws, &wire_id(id), None));

    assert_eq!(read.content, "");
    assert_eq!(read.revision, None);
    assert!(!read.conflict);
}

#[test]
fn a_written_draft_reads_back_with_its_revision() {
    let (ws, id, fs, _tmp) = workspace_with_drafts();

    let written = draft(handle_write_draft(Some(&fs), &ws, WriteDraftRequest {
        id: Some("w1".to_owned()),
        conversation: wire_id(id),
        content: "half a thought".to_owned(),
        revision: None,
    }));

    assert_eq!(written.id.as_deref(), Some("w1"));
    assert_eq!(written.content, "half a thought");
    assert!(!written.conflict);

    let read = draft(handle_read_draft(Some(&fs), &ws, &wire_id(id), None));

    assert_eq!(read.content, "half a thought");
    assert_eq!(
        read.revision, written.revision,
        "the revision a write reports is the one a read gives back"
    );
}

/// The whole point of the revision: a write based on a version that has since
/// moved on is refused, and the caller is handed what it had not seen.
#[test]
fn a_write_against_a_stale_revision_is_refused() {
    let (ws, id, fs, _tmp) = workspace_with_drafts();

    let first = draft(handle_write_draft(Some(&fs), &ws, WriteDraftRequest {
        id: None,
        conversation: wire_id(id),
        content: "what the other writer typed".to_owned(),
        revision: None,
    }));

    // A second writer that started from no draft at all, and so never saw the
    // first write.
    let refused = draft(handle_write_draft(Some(&fs), &ws, WriteDraftRequest {
        id: Some("w2".to_owned()),
        conversation: wire_id(id),
        content: "what I typed".to_owned(),
        revision: None,
    }));

    assert!(refused.conflict);
    assert_eq!(refused.id.as_deref(), Some("w2"));
    assert_eq!(
        refused.content, "what the other writer typed",
        "the refusal carries the text on disk, not the text submitted"
    );
    assert_eq!(refused.revision, first.revision);

    // And nothing was overwritten.
    let read = draft(handle_read_draft(Some(&fs), &ws, &wire_id(id), None));
    assert_eq!(read.content, "what the other writer typed");
}

/// An empty write removes the draft rather than leaving a blank file, which the
/// CLI would otherwise seed an editor from and treat as a recovery copy.
#[test]
fn an_empty_write_removes_the_draft() {
    let (ws, id, fs, _tmp) = workspace_with_drafts();

    let written = draft(handle_write_draft(Some(&fs), &ws, WriteDraftRequest {
        id: None,
        conversation: wire_id(id),
        content: "to be discarded".to_owned(),
        revision: None,
    }));

    let cleared = draft(handle_write_draft(Some(&fs), &ws, WriteDraftRequest {
        id: None,
        conversation: wire_id(id),
        content: String::new(),
        revision: written.revision,
    }));

    assert_eq!(cleared.content, "");
    assert_eq!(cleared.revision, None);
    assert!(!cleared.conflict);

    let path = draft_path(Some(&fs), &ws, &id, false);
    assert!(
        path.is_none_or(|path| !path.exists()),
        "the draft file is gone, not blank"
    );
}

/// Without user-local storage there is nowhere a draft may live, and saying so
/// beats writing it into the workspace where a teammate would see it.
#[test]
fn writing_a_draft_without_user_local_storage_fails() {
    let (ws, id, _tmp) = workspace_with_conversation();

    let response = handle_write_draft(None, &ws, WriteDraftRequest {
        id: Some("w3".to_owned()),
        conversation: wire_id(id),
        content: "nowhere to go".to_owned(),
        revision: None,
    });

    match response {
        HostToPlugin::Error(error) => {
            assert_eq!(error.request.as_deref(), Some("write_draft"));
            assert!(error.message.contains("user-local storage"), "{error:?}");
        }
        other => panic!("expected an error, got {other:?}"),
    }
}

/// A conversation that has been archived is no longer somewhere a draft can go:
/// `jp query` never looks beside the archive, and unarchiving deletes whatever
/// occupies the live path.
#[test]
fn writing_a_draft_for_an_archived_conversation_fails() {
    let (mut ws, id, fs, _tmp) = workspace_with_drafts();

    handle_archive(&mut ws, None, &wire_id(id), None);

    let response = handle_write_draft(Some(&fs), &ws, WriteDraftRequest {
        id: Some("w4".to_owned()),
        conversation: wire_id(id),
        content: "typed against a stale list".to_owned(),
        revision: None,
    });

    match response {
        HostToPlugin::Error(error) => {
            assert_eq!(error.request.as_deref(), Some("write_draft"));
            assert!(error.message.contains("does not exist"), "{error:?}");
        }
        other => panic!("expected an error, got {other:?}"),
    }

    assert!(
        !fs.build_conversation_dir(&id, None, true).exists(),
        "no live directory is left beside the archived one"
    );
}

/// The answer is the query text, while the revision covers the stored file: a
/// draft composed in an editor keeps its configuration and history sections on
/// disk, and neither reaches the plugin.
#[test]
fn a_stored_draft_reads_back_as_its_query_text() {
    let (ws, id, fs, _tmp) = workspace_with_drafts();
    let path = draft_path(Some(&fs), &ws, &id, true).unwrap();

    // The shape `jp q --edit` leaves on disk, built from the real marker so the
    // fixture cannot drift from the parser.
    let document = format!(
        "half a thought\n\n{CUT_MARKER}\n\n# Active \
         Configuration\n\n```toml\n[assistant.model]\nid = \"anthropic/claude\"\n```\n"
    );

    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, &document).unwrap();

    let read = draft(handle_read_draft(Some(&fs), &ws, &wire_id(id), None));
    assert_eq!(
        read.content, "half a thought",
        "the configuration section stays on disk"
    );

    let written = draft(handle_write_draft(Some(&fs), &ws, WriteDraftRequest {
        id: None,
        conversation: wire_id(id),
        content: "half a thought, finished".to_owned(),
        revision: read.revision,
    }));

    assert!(
        !written.conflict,
        "the revision covers the bytes the answer left out, and a write based on it is still \
         accepted"
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "half a thought, finished"
    );
}

/// The conversation index is a snapshot from startup, so a host that stays up
/// while another process archives a conversation has to ask the store before
/// writing a draft nothing would read.
#[test]
fn writing_a_draft_for_a_conversation_archived_elsewhere_fails() {
    let (ws, id, fs, _tmp) = workspace_with_drafts();

    // Another process archives it, leaving this host's index stale.
    fs.archive(&id).unwrap();
    assert!(
        ws.acquire_conversation(&id).is_ok(),
        "the host still believes the conversation is live, which is the point"
    );

    let response = handle_write_draft(Some(&fs), &ws, WriteDraftRequest {
        id: Some("w5".to_owned()),
        conversation: wire_id(id),
        content: "typed against a stale list".to_owned(),
        revision: None,
    });

    match response {
        HostToPlugin::Error(error) => {
            assert_eq!(error.request.as_deref(), Some("write_draft"));
            assert!(error.message.contains("does not exist"), "{error:?}");
        }
        other => panic!("expected an error, got {other:?}"),
    }

    assert!(
        !fs.build_conversation_dir(&id, None, true).exists(),
        "no live directory is left beside the archived one"
    );
}

/// A draft the host cannot read is not an absent draft.
/// Reporting it as one would have a plugin compose from nothing, and then
/// overwrite text it never saw.
#[test]
fn an_unreadable_draft_is_reported_rather_than_read_as_empty() {
    let (ws, id, fs, _tmp) = workspace_with_drafts();
    let path = draft_path(Some(&fs), &ws, &id, true).unwrap();

    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, [0xff, 0xfe]).unwrap();

    match handle_read_draft(Some(&fs), &ws, &wire_id(id), Some("r5".to_owned())) {
        HostToPlugin::Error(error) => {
            assert_eq!(error.id.as_deref(), Some("r5"));
            assert_eq!(error.request.as_deref(), Some("read_draft"));
        }
        other => panic!("expected an error, got {other:?}"),
    }

    let response = handle_write_draft(Some(&fs), &ws, WriteDraftRequest {
        id: None,
        conversation: wire_id(id),
        content: "mine".to_owned(),
        revision: None,
    });

    match response {
        HostToPlugin::Error(error) => assert_eq!(error.request.as_deref(), Some("write_draft")),
        other => panic!("expected an error, got {other:?}"),
    }

    assert_eq!(
        std::fs::read(&path).unwrap(),
        [0xff, 0xfe],
        "the bytes the host could not read are still there"
    );
}

#[test]
fn set_title_names_a_conversation() {
    let (ws, id, _tmp) = workspace_with_conversation();

    let response = handle_set_title(&ws, None, SetTitleRequest {
        id: Some("r1".to_owned()),
        conversation: wire_id(id),
        title: Some("  Tool call header misaligns  ".to_owned()),
    });

    assert_eq!(
        response,
        HostToPlugin::Done(DoneResponse {
            id: Some("r1".to_owned())
        })
    );

    let handle = ws.acquire_conversation(&id).unwrap();
    assert_eq!(
        ws.metadata(&handle).unwrap().title.as_deref(),
        Some("Tool call header misaligns"),
        "the title is stored trimmed"
    );
}

/// A blank title clears the name rather than storing an empty one, which leaves
/// the conversation eligible for a generated title again.
#[test]
fn a_blank_set_title_clears_the_name() {
    let (ws, id, _tmp) = workspace_with_conversation();

    handle_set_title(&ws, None, SetTitleRequest {
        id: None,
        conversation: wire_id(id),
        title: Some("Named".to_owned()),
    });

    handle_set_title(&ws, None, SetTitleRequest {
        id: None,
        conversation: wire_id(id),
        title: Some("   ".to_owned()),
    });

    let handle = ws.acquire_conversation(&id).unwrap();
    assert_eq!(ws.metadata(&handle).unwrap().title, None);
}

#[test]
fn archiving_takes_a_conversation_out_of_the_index() {
    let (mut ws, id, _tmp) = workspace_with_conversation();

    let response = handle_archive(&mut ws, None, &wire_id(id), Some("r2".to_owned()));

    assert_eq!(
        response,
        HostToPlugin::Done(DoneResponse {
            id: Some("r2".to_owned())
        })
    );
    assert!(
        ws.acquire_conversation(&id).is_err(),
        "an archived conversation is no longer in the index"
    );
}

/// A store that will not take the write answers `error`, not `done`.
///
/// The write happens under a lock, and a persist failure has no other way to
/// reach the plugin: nothing about the conversation on disk says the title
/// changed, so `done` would be the only thing it ever learned.
#[test]
fn a_failed_write_is_reported_rather_than_confirmed() {
    let (ws, id, _tmp) = workspace_with_conversation();
    let ws = ws.with_persist(Arc::new(RefusingBackend));

    let response = handle_set_title(&ws, None, SetTitleRequest {
        id: Some("r4".to_owned()),
        conversation: wire_id(id),
        title: Some("Never stored".to_owned()),
    });

    match response {
        HostToPlugin::Error(error) => {
            assert_eq!(error.id.as_deref(), Some("r4"));
            assert_eq!(error.request.as_deref(), Some("set_title"));
            assert!(
                error.message.contains("failed to save the title"),
                "{error:?}"
            );
        }
        other => panic!("expected an error, got {other:?}"),
    }
}

/// A persist backend that refuses every write.
#[derive(Debug)]
struct RefusingBackend;

impl jp_storage::backend::PersistBackend for RefusingBackend {
    fn write(
        &self,
        _id: &ConversationId,
        _metadata: &Conversation,
        _events: &jp_conversation::ConversationStream,
        _projection: jp_storage::backend::Projection,
    ) -> Result<(), jp_storage::Error> {
        Err(jp_storage::Error::write_failed(
            "/read-only/events.json",
            std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        ))
    }

    fn remove(&self, _id: &ConversationId) -> Result<(), jp_storage::Error> {
        Ok(())
    }

    fn archive(&self, _id: &ConversationId) -> Result<(), jp_storage::Error> {
        Ok(())
    }

    fn unarchive(&self, _id: &ConversationId) -> Result<(), jp_storage::Error> {
        Ok(())
    }
}

/// A failure names the request it belongs to, so a plugin with several in
/// flight can tell which one it answers.
#[test]
fn an_unknown_conversation_fails_against_its_request() {
    let mut ws = bare_workspace();

    let response = handle_archive(&mut ws, None, "not-an-id", Some("r3".to_owned()));

    match response {
        HostToPlugin::Error(error) => {
            assert_eq!(error.id.as_deref(), Some("r3"));
            assert_eq!(error.request.as_deref(), Some("archive_conversation"));
            assert!(
                error.message.contains("invalid conversation ID"),
                "{error:?}"
            );
        }
        other => panic!("expected an error, got {other:?}"),
    }
}

/// The messages the host wrote back, in the order the plugin receives them.
fn replies(sink: &[u8]) -> Vec<HostToPlugin> {
    String::from_utf8(sink.to_vec())
        .expect("the host writes utf-8")
        .lines()
        .map(|line| serde_json::from_str(line).expect("a host message"))
        .collect()
}

/// A long-running host sees what another process wrote after it started.
///
/// The host loads the index once at startup.
/// Without re-reading it, a plugin asking for the conversation list is served
/// that snapshot for the life of the process, so a conversation started in a
/// terminal never appears.
#[tokio::test]
async fn a_conversation_written_after_startup_is_listed() {
    let (mut ws, _first, fs, tmp) = workspace_with_drafts();
    let mut sink: Vec<u8> = Vec::new();

    // The host's view, taken at startup.
    ws.load_conversation_index();
    assert_eq!(ws.conversations().count(), 1);

    // Another process writes a second conversation. Same store, its own handle,
    // which is what a `jp query` in a terminal amounts to.
    let second = conversation_id(1_700_000_001);
    fs.write_test_conversation(&second, &Conversation::default());

    let response = handle_request(
        PluginToHost::ListConversations(OptionalId { id: None }),
        &mut sink,
        &mut ws,
        &json!({}),
        None,
        None,
        &AppConfig::new_test(),
    )
    .unwrap();
    assert_eq!(response, Flow::Continue);

    // Asserted against what reached the plugin rather than the workspace: a
    // refresh running after the response is serialized leaves the workspace
    // right and the plugin holding the stale list.
    let sent = replies(&sink);
    let [HostToPlugin::Conversations(listed)] = sent.as_slice() else {
        panic!("expected one conversations response, got {sent:?}");
    };

    // Sorted because the index is a map, and the order it iterates in is not
    // what this test is about.
    let mut ids: Vec<&str> = listed.data.iter().map(|c| c.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(ids, ["jp-c17000000000", "jp-c17000000010"]);

    drop(tmp);
}

/// A conversation the host has already read is read again, not served from the
/// copy it kept.
///
/// The first read loads the stream and caches it.
/// A `jp query` in a terminal appends to the same conversation, and without
/// dropping that cache the plugin is served the events as they stood before
/// that turn ran, for the life of the process.
#[tokio::test]
async fn an_event_written_after_a_read_is_served_by_the_next_read() {
    let (mut ws, id, fs, tmp) = workspace_with_drafts();
    let mut sink: Vec<u8> = Vec::new();

    let request = || {
        PluginToHost::ReadEvents(ReadEventsRequest {
            id: None,
            conversation: wire_id(id),
        })
    };

    // The read that populates the host's stream cache.
    handle_request(
        request(),
        &mut sink,
        &mut ws,
        &json!({}),
        None,
        None,
        &AppConfig::new_test(),
    )
    .unwrap();

    // Another process runs a turn on that same conversation.
    let stream = ConversationStream::new_test().with_turn("what the other terminal asked");
    fs.write(
        &id,
        &Conversation::default(),
        &stream,
        Projection::Projected,
    )
    .unwrap();

    handle_request(
        request(),
        &mut sink,
        &mut ws,
        &json!({}),
        None,
        None,
        &AppConfig::new_test(),
    )
    .unwrap();

    let sent = replies(&sink);
    let [HostToPlugin::Events(before), HostToPlugin::Events(after)] = sent.as_slice() else {
        panic!("expected two events responses, got {sent:?}");
    };

    assert!(
        before.data.is_empty(),
        "the conversation had no events when it was first read: {before:?}"
    );

    let [turn_start, chat_request] = after.data.as_slice() else {
        panic!("expected the turn the other process wrote, got {after:?}");
    };
    assert_eq!(turn_start["type"], "turn_start");
    assert_eq!(chat_request["type"], "chat_request");
    assert_eq!(chat_request["content"], "what the other terminal asked");

    drop(tmp);
}

/// The canonical spelling is what JP prints, and bare deciseconds still resolve
/// because that is what the wire carried before.
#[test]
fn a_conversation_is_named_the_way_jp_prints_it() {
    let (ws, id, _tmp) = workspace_with_conversation();

    assert_eq!(
        wire_id(id),
        "jp-c17000000000",
        "a plugin is handed the spelling a user can paste back into `jp`"
    );

    assert_eq!(parse_conversation_id("jp-c17000000000").unwrap(), id);
    assert_eq!(parse_conversation_id("17000000000").unwrap(), id);
    assert!(parse_conversation_id("not-an-id").is_err());

    // Asserted exactly, not round-tripped: both spellings parse, so a
    // round-trip would pass whichever one the host emitted.
    let HostToPlugin::Conversations(listed) = handle_list_conversations(&ws, None) else {
        panic!("expected a conversations response");
    };
    let [summary] = listed.data.as_slice() else {
        panic!("expected exactly one conversation");
    };
    assert_eq!(summary.id, "jp-c17000000000");
}

#[tokio::test]
async fn a_ready_carries_on_and_a_clean_exit_stops() {
    let mut ws = bare_workspace();
    let config = json!({});
    let mut sink: Vec<u8> = Vec::new();

    assert_eq!(
        handle_request(
            PluginToHost::Ready(ReadyMessage { protocol: 1 }),
            &mut sink,
            &mut ws,
            &config,
            None,
            None,
            &AppConfig::new_test(),
        )
        .unwrap(),
        Flow::Continue
    );

    assert_eq!(
        handle_request(
            PluginToHost::Exit(ExitMessage {
                code: 0,
                reason: None,
            }),
            &mut sink,
            &mut ws,
            &config,
            None,
            None,
            &AppConfig::new_test(),
        )
        .unwrap(),
        Flow::Stop
    );
}

/// A non-zero exit is the plugin's failure, so it surfaces as one rather than
/// ending the run quietly.
#[tokio::test]
async fn a_failing_exit_carries_its_code_and_reason() {
    let mut ws = bare_workspace();
    let mut sink: Vec<u8> = Vec::new();

    let error = handle_request(
        PluginToHost::Exit(ExitMessage {
            code: 3,
            reason: Some("no such ticket".to_owned()),
        }),
        &mut sink,
        &mut ws,
        &json!({}),
        None,
        None,
        &AppConfig::new_test(),
    )
    .expect_err("a non-zero exit is an error");

    assert_eq!(error.code.get(), 3);
    assert_eq!(error.message.as_deref(), Some("no such ticket"));
}

/// A plugin that ignores `Shutdown` is killed rather than waited on forever,
/// and so is every process it started.
///
/// The host holds the only handles to the plugin's stdin — this scope and the
/// shutdown thread — so a plugin blocked on a read never sees EOF and never
/// exits on its own.
/// Waiting on one is a wait with no end, and it takes the error that caused it
/// down with it.
///
/// The plugin here is a shell script running a worker, which is exactly that
/// plugin: it reads nothing and exits on nothing short of a signal.
///
/// The worker inherits the plugin's stderr, so the pipe closing is how the test
/// sees the worker die.
/// Its PID is no evidence either way: once the plugin is gone the worker
/// belongs to PID 1, and a container whose PID 1 does not reap leaves it a
/// zombie that still answers to its PID.
#[cfg(unix)]
#[test]
fn stop_plugin_kills_a_plugin_and_its_workers() {
    use std::{os::unix::fs::PermissionsExt as _, sync::mpsc};

    let dir = tempdir().unwrap();
    let script = dir.path().join("jp-worker");
    fs::write(&script, "#!/bin/sh\nsleep 600 &\necho ready\nwait\n").unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

    let PluginProcess {
        mut child,
        tree,
        stdin,
        stdout,
        stderr_handle,
    } = spawn_plugin(&script, None).unwrap();

    // Waited for, so the worker exists before the plugin is told to stop.
    let mut line = String::new();
    BufReader::new(stdout).read_line(&mut line).unwrap();
    assert_eq!(line, "ready\n");
    let plugin = tree.pid();

    // On its own thread with a deadline: if `stop_plugin` ever goes back to
    // waiting indefinitely, this fails rather than hanging the suite — which is
    // the failure mode being guarded against.
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let sent = AtomicBool::new(false);
        stop_plugin(&stdin, &sent, &tree, Duration::from_millis(200));
        let _ = tx.send(());
    });

    rx.recv_timeout(Duration::from_secs(10))
        .expect("stop_plugin returned rather than waiting forever");

    // Killed, not finished: left alone, the child runs for a minute and exits
    // successfully.
    let status = child.wait().unwrap();
    assert!(!status.success(), "the plugin was killed: {status:?}");
    assert!(
        !is_process_alive(plugin),
        "a plugin that ignored the request is gone"
    );

    assert!(
        join_within(stderr_handle, Duration::from_secs(5)),
        "the plugin's worker is killed with it, so nothing holds its stderr open"
    );
}

/// A plugin that ignores `Shutdown` is killed, and so is every process it
/// started.
///
/// The plugin is a batch file that starts a worker in the background and then
/// waits in the foreground.
/// Both inherit the plugin's stderr, so the pipe closing is how the test sees
/// the worker die.
///
/// The worker starts after the plugin is resumed, so this covers the job being
/// killed, not the window between spawn and joining the job: no test can make a
/// plugin reliably win that race.
#[cfg(windows)]
#[test]
fn stop_plugin_kills_a_plugin_and_its_workers() {
    use std::sync::mpsc;

    let dir = tempdir().unwrap();
    let script = dir.path().join("jp-worker.bat");
    fs::write(
        &script,
        "@echo off\r\nstart /b \"\" ping -n 600 127.0.0.1 >nul\r\necho ready\r\nping -n 600 \
         127.0.0.1 >nul\r\n",
    )
    .unwrap();

    let PluginProcess {
        mut child,
        tree,
        stdin,
        stdout,
        stderr_handle,
    } = spawn_plugin(&script, None).unwrap();

    // Waited for, so the worker exists before the plugin is told to stop.
    let mut line = String::new();
    BufReader::new(stdout).read_line(&mut line).unwrap();
    assert_eq!(line, "ready\r\n");
    let plugin = tree.pid();

    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let sent = AtomicBool::new(false);
        stop_plugin(&stdin, &sent, &tree, Duration::from_millis(200));
        let _ = tx.send(());
    });

    rx.recv_timeout(Duration::from_secs(10))
        .expect("stop_plugin returned rather than waiting forever");

    assert!(child.wait().is_ok(), "the plugin is reaped");
    assert!(
        !is_process_alive(plugin),
        "a plugin that ignored the request is gone"
    );

    assert!(
        join_within(stderr_handle, Duration::from_secs(5)),
        "the plugin's worker is killed with it, so nothing holds its stderr open"
    );
}

/// A thread that never finishes is left behind rather than waited on.
#[test]
fn join_within_gives_up_on_a_thread_that_does_not_finish() {
    let (release, held) = std::sync::mpsc::channel::<()>();
    let blocked = thread::spawn(move || {
        let _ = held.recv();
    });

    assert!(!join_within(blocked, Duration::from_millis(50)));

    drop(release);
}

#[test]
fn join_within_joins_a_thread_that_finishes() {
    let finished = thread::spawn(|| {});

    assert!(join_within(finished, Duration::from_secs(5)));
}

#[test]
fn handle_read_config_full() {
    let config = json!({"assistant": {"name": "JP"}, "style": {"code": {}}});
    let resp = handle_read_config(&config, None, None);

    if let HostToPlugin::Config(cfg) = resp {
        assert_eq!(cfg.data, config);
        assert!(cfg.path.is_none());
    } else {
        panic!("expected Config response");
    }
}

#[test]
fn handle_read_config_path() {
    let config = json!({"assistant": {"name": "JP", "model": {"id": "test"}}});
    let resp = handle_read_config(
        &config,
        Some("assistant.model".to_owned()),
        Some("r1".to_owned()),
    );

    if let HostToPlugin::Config(cfg) = resp {
        assert_eq!(cfg.data, json!({"id": "test"}));
        assert_eq!(cfg.path.as_deref(), Some("assistant.model"));
        assert_eq!(cfg.id.as_deref(), Some("r1"));
    } else {
        panic!("expected Config response");
    }
}

#[test]
fn handle_read_config_invalid_path() {
    let config = json!({"assistant": {"name": "JP"}});
    let resp = handle_read_config(&config, Some("nonexistent.path".to_owned()), None);

    assert!(matches!(resp, HostToPlugin::Error(_)));
}

/// A workspace whose `.jp/config.toml` names the assistant `name`.
///
/// The user-global config directory is pointed at an empty directory, so the
/// configuration of whoever runs the tests stays out of the result.
fn workspace_with_config(
    name: &str,
) -> (Workspace, Arc<FsStorageBackend>, Utf8TempDir, EnvVarGuard) {
    let tmp = tempdir().unwrap();
    let env = EnvVarGuard::set("JP_GLOBAL_CONFIG_DIR", tmp.path().join("global").as_str());

    let fs = Arc::new(FsStorageBackend::new(&tmp.path().join(".jp")).unwrap());
    write_workspace_config(&tmp, name);

    let workspace = Workspace::in_memory(tmp.path()).with_backend(fs.clone());
    (workspace, fs, tmp, env)
}

fn write_workspace_config(tmp: &Utf8TempDir, name: &str) {
    fs::write(
        tmp.path().join(".jp/config.toml"),
        format!(
            "[assistant]\nname = \"{name}\"\nmodel.id = \
             \"anthropic/test\"\n\n[conversation.tools.'*']\nrun = \"ask\"\n"
        ),
    )
    .unwrap();
}

/// A long-running plugin polls with `reload` to see an edit made while it runs.
#[test]
#[serial(env_vars)]
fn a_reload_reads_a_config_file_changed_since_startup() {
    let (workspace, fs, tmp, _env) = workspace_with_config("before");

    let first = reload_config(&[], &workspace, Some(&fs), tmp.path()).unwrap();
    assert_eq!(first["assistant"]["name"], json!("before"));

    write_workspace_config(&tmp, "after");

    let second = reload_config(&[], &workspace, Some(&fs), tmp.path()).unwrap();
    assert_eq!(second["assistant"]["name"], json!("after"));
}

/// The invocation's `--cfg` arguments still win over the files they are layered
/// on, as they did when the plugin started.
#[test]
#[serial(env_vars)]
fn a_reload_layers_the_invocation_cfg_arguments_over_the_files() {
    let (workspace, fs, tmp, _env) = workspace_with_config("from-file");
    let overrides = vec!["assistant.name=from-cfg".parse::<KeyValueOrPath>().unwrap()];

    let config = reload_config(&overrides, &workspace, Some(&fs), tmp.path()).unwrap();

    assert_eq!(config["assistant"]["name"], json!("from-cfg"));
}

/// A file saved halfway through an edit is reported, not read as empty.
#[test]
#[serial(env_vars)]
fn a_reload_of_an_unparseable_config_file_is_an_error() {
    let (workspace, fs, tmp, _env) = workspace_with_config("before");
    fs::write(tmp.path().join(".jp/config.toml"), "[assistant\nname = ").unwrap();

    let error = reload_config(&[], &workspace, Some(&fs), tmp.path()).unwrap_err();

    // The parser's diagnostic says what is wrong and where in the file. It
    // sits below a bare `Configuration error`, which on its own tells the
    // plugin nothing to pass on.
    assert_eq!(
        error,
        "failed to read the workspace configuration: Configuration error: TOML parse error at \
         line 1, column 11\n  |\n1 | [assistant\n  |           ^\nunclosed table, expected `]`\n"
    );
}

/// The options `init` carries are a plain map of option to value, and a plugin
/// reading them again through `read_config` has to find the same shape.
#[test]
#[serial(env_vars)]
fn read_config_returns_plugin_options_in_the_shape_init_sends() {
    let (workspace, fs, tmp, _env) = workspace_with_config("jp");
    let path = tmp.path().join(".jp/config.toml");
    let mut toml = fs::read_to_string(&path).unwrap();
    toml.push_str("\n[plugins.command.ticket.options]\ndir = \"docs/ticket\"\n");
    fs::write(&path, toml).unwrap();

    let config = reload_config(&[], &workspace, Some(&fs), tmp.path()).unwrap();

    assert_eq!(
        config["plugins"]["command"]["ticket"]["options"],
        json!({"dir": "docs/ticket"})
    );
}

/// Options declared with an explicit merge strategy are still just options to
/// the plugin: the strategy steers how config layers combine, and is not part
/// of what the plugin was configured with.
#[test]
#[serial(env_vars)]
fn read_config_returns_plugin_options_without_their_merge_strategy() {
    let (workspace, fs, tmp, _env) = workspace_with_config("jp");
    let path = tmp.path().join(".jp/config.toml");
    let mut toml = fs::read_to_string(&path).unwrap();
    toml.push_str(
        "\n[plugins.command.ticket]\noptions = { value = { dir = \"docs/ticket\" }, strategy = \
         \"replace\" }\n",
    );
    fs::write(&path, toml).unwrap();

    let config = reload_config(&[], &workspace, Some(&fs), tmp.path()).unwrap();

    assert_eq!(
        config["plugins"]["command"]["ticket"]["options"],
        json!({"dir": "docs/ticket"})
    );
}

/// An error whose `Display` says one thing and whose source says another.
#[derive(Debug)]
struct Layered {
    message: &'static str,
    source: Option<Box<Layered>>,
}

impl std::fmt::Display for Layered {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message)
    }
}

impl std::error::Error for Layered {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn std::error::Error + 'static))
    }
}

fn layered(message: &'static str, source: Option<Layered>) -> Layered {
    Layered {
        message,
        source: source.map(Box::new),
    }
}

/// The outermost message is a category, so sending it alone tells the reader
/// nothing.
/// A plugin cannot ask for the sources, so they are flattened in.
#[test]
fn an_error_is_reported_with_its_causes() {
    let error = layered(
        "LLM error",
        Some(layered(
            "request failed",
            Some(layered(
                "prompt is too long: 1000327 tokens > 1000000 maximum",
                None,
            )),
        )),
    );

    assert_eq!(
        error_chain(&error),
        "LLM error: request failed: prompt is too long: 1000327 tokens > 1000000 maximum"
    );
}

/// A wrapper that already quotes its source should not say it twice.
#[test]
fn a_restated_cause_is_not_repeated() {
    let error = layered(
        "config error: no such model `haiku`",
        Some(layered("no such model `haiku`", None)),
    );

    assert_eq!(error_chain(&error), "config error: no such model `haiku`");
}

#[test]
fn a_lone_error_is_reported_as_itself() {
    assert_eq!(
        error_chain(&layered("nothing underneath", None)),
        "nothing underneath"
    );
}

/// A plugin needing a newer protocol than this host is refused on its `ready`,
/// before it can send anything the host would fail to parse.
#[tokio::test]
async fn a_plugin_needing_a_newer_protocol_is_refused() {
    let mut ws = bare_workspace();
    let mut sink: Vec<u8> = Vec::new();

    let error = handle_request(
        PluginToHost::Ready(ReadyMessage {
            protocol: PROTOCOL_VERSION + 1,
        }),
        &mut sink,
        &mut ws,
        &json!({}),
        None,
        None,
        &AppConfig::new_test(),
    )
    .expect_err("a plugin needing a newer protocol must be refused");

    assert!(
        error.to_string().contains("Reinstall the two together"),
        "{error}"
    );
}

/// An interrupt reaches the turn it names and no other.
///
/// The failure this guards against is stopping the wrong turn: a host runs
/// several at once, and the request is the only thing that says which.
#[test]
fn an_interrupt_reaches_only_the_turn_it_names() {
    let turns = RunningTurns::default();
    let wanted = conversation_id(1_700_000_000);
    let other = conversation_id(1_700_000_001);

    let mut wanted_rx = turns.register(wanted);
    let mut other_rx = turns.register(other);

    turns.interrupt(wanted, InterruptAction::Stop).unwrap();

    assert_eq!(wanted_rx.try_next(), Some(InterruptAction::Stop));
    assert_eq!(other_rx.try_next(), None);
}

/// A turn that has finished is reported as gone rather than silently accepting
/// an interrupt nothing will read.
///
/// This is what lets a client tell "interrupted" from "it had already
/// finished", and send its message as a new turn instead of losing it.
#[test]
fn interrupting_a_finished_turn_says_so() {
    let turns = RunningTurns::default();
    let id = conversation_id(1_700_000_000);

    let _rx = turns.register(id);
    turns.interrupt(id, InterruptAction::Stop).unwrap();

    turns.finished(id);

    let error = turns
        .interrupt(id, InterruptAction::Stop)
        .expect_err("the turn is gone");

    assert_eq!(
        error,
        format!("no turn is running on conversation {id}"),
        "the message names the conversation, because a client can be watching several"
    );
}

/// A turn whose receiver is gone is reported as ended, not as never having
/// existed: the entry is still registered, so the two are distinguishable.
#[test]
fn interrupting_a_turn_that_stopped_reading_says_it_ended() {
    let turns = RunningTurns::default();
    let id = conversation_id(1_700_000_000);

    drop(turns.register(id));

    let error = turns
        .interrupt(id, InterruptAction::Stop)
        .expect_err("nothing is reading");

    assert_eq!(error, format!("the turn on conversation {id} has ended"));
}

/// The wire's `reply` carries the text the turn answers with.
#[test]
fn a_reply_request_carries_its_content_to_the_turn() {
    let action = requested_action(&InterruptRequest::reply(
        "jp-c17000000000".to_owned(),
        "  use Rust instead  ".to_owned(),
    ))
    .expect("a reply with content is deliverable");

    assert_eq!(action, InterruptAction::Reply {
        // Trimmed, because a browser's textarea keeps the newline a user
        // pressed Enter on before deciding to send.
        content: "use Rust instead".to_owned(),

        // Nobody watched this arrive at the terminal the turn runs in.
        echo: true,
    });
}

/// A `reply` with nothing to say is refused rather than delivered as an empty
/// message the assistant has to answer.
#[test]
fn a_reply_request_without_content_is_refused() {
    let blank = InterruptRequest {
        content: Some("   \n ".to_owned()),
        ..InterruptRequest::reply("jp-c17000000000".to_owned(), String::new())
    };

    assert_eq!(
        requested_action(&blank).expect_err("a blank reply is not deliverable"),
        "a reply needs `content`"
    );

    let missing = InterruptRequest {
        action: WireAction::Reply,
        ..InterruptRequest::stop("jp-c17000000000".to_owned())
    };

    assert_eq!(
        requested_action(&missing).expect_err("a reply with no content field is not deliverable"),
        "a reply needs `content`"
    );
}

/// An `interrupt` from a protocol 8 plugin carries no action, and means the
/// stop it meant then.
#[test]
fn an_interrupt_without_an_action_stops_the_turn() {
    let request: InterruptRequest =
        serde_json::from_value(json!({ "conversation": "jp-c17000000000" })).unwrap();

    assert_eq!(requested_action(&request).unwrap(), InterruptAction::Stop);
}

/// A request that asked for an answer gets one once the turn has taken it; one
/// that did not is left alone.
///
/// An uncorrelated response is a message the plugin has nowhere to put, and the
/// dispatcher pairs replies to requests by id.
#[tokio::test]
async fn only_an_interrupt_with_an_id_is_answered() {
    let turns = RunningTurns::default();
    let id = conversation_id(1_700_000_000);
    let mut rx = turns.register(id);

    assert!(
        handle_interrupt(InterruptRequest::stop(id.to_string()), &turns).is_none(),
        "a fire-and-forget interrupt is not answered"
    );
    assert_eq!(rx.try_next(), Some(InterruptAction::Stop));

    let answer = handle_interrupt(
        InterruptRequest::stop(id.to_string()).with_id("req-1".to_owned()),
        &turns,
    )
    .expect("a request with an id is answered");

    // The turn acts on it, and only then is it reported delivered.
    assert_eq!(rx.try_next(), Some(InterruptAction::Stop));

    assert_eq!(
        answer.await,
        HostToPlugin::Done(DoneResponse {
            id: Some("req-1".to_owned())
        })
    );
}

/// A reply queued for a turn that ends without reading it is refused, not
/// reported delivered.
///
/// The turn can pass its last read and still be registered for a moment, so
/// queueing succeeds; the refusal is what tells the client to send the message
/// as a turn of its own rather than lose it.
#[tokio::test]
async fn a_reply_the_turn_never_read_is_refused() {
    let turns = RunningTurns::default();
    let id = conversation_id(1_700_000_000);
    let rx = turns.register(id);

    let answer = handle_interrupt(
        InterruptRequest::reply(id.to_string(), "use Rust instead".to_owned())
            .with_id("req-1".to_owned()),
        &turns,
    )
    .expect("a request with an id is answered");

    // The turn ends with the reply still queued.
    drop(rx);
    turns.finished(id);

    assert_eq!(
        answer.await,
        HostToPlugin::Error(ErrorResponse {
            id: Some("req-1".to_owned()),
            request: Some("interrupt".to_owned()),
            message: format!("the turn on conversation {id} ended before acting on this"),
        })
    );
}

/// A turn that finishes while its title is still being generated answers
/// straight away, and hands the title on rather than waiting for it.
///
/// Waiting would keep the conversation locked, and the client's next message
/// would be refused as already-locked for as long as the title model took.
#[tokio::test]
async fn a_turn_does_not_wait_for_its_title() {
    let written = Arc::new(Mutex::new(Vec::<String>::new()));

    let (outcome, outstanding) = tokio::time::timeout(
        Duration::from_secs(5),
        beside_title(
            async { "answered" },
            Some(Box::pin(std::future::pending())),
            |title| written.lock().unwrap().push(title),
        ),
    )
    .await
    .expect("the turn's outcome must not wait on the title");

    assert_eq!(outcome, "answered");
    assert!(outstanding.is_some(), "the title is handed on");
    assert!(written.lock().unwrap().is_empty());
}

/// A title that arrives while the turn runs is written through the turn's own
/// lock, and nothing is left outstanding.
#[tokio::test]
async fn a_title_that_arrives_first_is_written_by_the_turn() {
    let written = Arc::new(Mutex::new(Vec::<String>::new()));
    let (finish, finished) = tokio::sync::oneshot::channel::<()>();

    let turn = async move {
        finished.await.unwrap();
        "answered"
    };
    let title: TitleFuture = Box::pin(async move {
        finish.send(()).unwrap();
        Some("Rust or Python".to_owned())
    });

    let (outcome, outstanding) = beside_title(turn, Some(title), |title| {
        written.lock().unwrap().push(title);
    })
    .await;

    assert_eq!(outcome, "answered");
    assert!(outstanding.is_none());
    assert_eq!(*written.lock().unwrap(), ["Rust or Python"]);
}

/// A title that finishes after its turn, while the next turn on the same
/// conversation holds it, is written when that turn lets go rather than lost.
#[test]
fn a_late_title_waits_for_a_busy_conversation() {
    let (ws, id, _tmp) = workspace_with_conversation();
    let (titles, _arrived) = Titles::new();
    let title_of = |ws: &Workspace| {
        let handle = ws.acquire_conversation(&id).unwrap();
        ws.metadata(&handle).unwrap().title.clone()
    };

    let handle = ws.acquire_conversation(&id).unwrap();
    let LockResult::Acquired(next_turn) = ws.lock_conversation(handle, None).unwrap() else {
        panic!("the conversation starts unlocked");
    };

    titles.offer(&ws, ArrivedTitle {
        conversation: id,
        title: "Rust or Python".to_owned(),
    });
    assert_eq!(title_of(&ws), None, "the busy conversation is left alone");

    titles.release(next_turn);
    assert_eq!(title_of(&ws).as_deref(), Some("Rust or Python"));
}

/// A generated title never replaces one the user set while the model worked.
#[test]
fn a_late_title_does_not_replace_a_chosen_one() {
    let (ws, id, _tmp) = workspace_with_conversation();
    let (titles, _arrived) = Titles::new();

    handle_set_title(&ws, None, SetTitleRequest {
        id: None,
        conversation: wire_id(id),
        title: Some("Chosen".to_owned()),
    });

    titles.offer(&ws, ArrivedTitle {
        conversation: id,
        title: "Generated".to_owned(),
    });

    let handle = ws.acquire_conversation(&id).unwrap();
    assert_eq!(
        ws.metadata(&handle).unwrap().title.as_deref(),
        Some("Chosen")
    );
}

/// The user-global root is `<config dir>/config/`, the directory `--cfg` itself
/// searches, so a listing built from its parent reports nothing.
///
/// `JP_GLOBAL_CONFIG_DIR` also has to reach this handler: it is honoured
/// wherever else the roots are built.
#[test]
#[serial(env_vars)]
fn list_configs_reports_the_user_global_root() {
    let tmp = tempdir().unwrap();
    let global_dir = tmp.path().join("global");
    let _env = EnvVarGuard::set("JP_GLOBAL_CONFIG_DIR", global_dir.as_str());

    let path = global_dir.join("config/profiles/skill/rfd.toml");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "").unwrap();

    let mut config = AppConfig::new_test();
    config.config_load_paths = vec![RelativePathBuf::from("profiles")];

    let response = handle_list_configs(
        &config,
        &Workspace::in_memory(tmp.path().join("workspace")),
        None,
        None,
    );

    let HostToPlugin::Configs(configs) = response else {
        panic!("expected a configs response, got {response:?}");
    };

    assert_eq!(configs.data, vec![ConfigEntry {
        segment: "skill/rfd".to_owned(),
        namespace: "skill".to_owned(),
        name: "rfd".to_owned(),
    }]);
}

/// A `query` asking for `new` and, optionally, an expiry.
fn expiring_query(new: bool, expires_in: Option<&str>) -> QueryRequest {
    QueryRequest {
        id: None,
        conversation: String::new(),
        content: "Summarize".to_owned(),
        new,
        title: None,
        cfg: vec![],
        schema: None,
        expires_in: expires_in.map(str::to_owned),
    }
}

fn at(secs: i64) -> DateTime<Utc> {
    DateTime::<Utc>::UNIX_EPOCH + TimeDelta::seconds(secs)
}

#[test]
fn a_query_without_expires_in_creates_a_lasting_conversation() {
    assert_eq!(
        query_expiry(&expiring_query(true, None), at(1_700_000_000)),
        Ok(None)
    );
}

#[test]
fn expires_in_counts_from_when_the_conversation_is_created() {
    assert_eq!(
        query_expiry(&expiring_query(true, Some("5m")), at(1_700_000_000)),
        Ok(Some(at(1_700_000_300)))
    );
}

/// `jp query --tmp` requires `--new`, and silently keeping a conversation the
/// plugin asked to be temporary is the failure a refusal avoids.
#[test]
fn expires_in_on_an_existing_conversation_is_refused() {
    assert_eq!(
        query_expiry(&expiring_query(false, Some("5m")), at(1_700_000_000)),
        Err("`expires_in` applies only to a conversation the query creates".to_owned())
    );
}

#[test]
fn an_unparseable_expires_in_is_refused() {
    let error = query_expiry(&expiring_query(true, Some("soon")), at(1_700_000_000)).unwrap_err();

    assert!(
        error.starts_with(r#"invalid `expires_in` "soon": "#),
        "the message names the field and the value: {error}"
    );
}

/// Append a turn that ends in `response`, the way a query's turn does.
fn add_turn(stream: &mut ConversationStream, response: ChatResponse) {
    stream.start_turn(ChatRequest::from("Summarize"));
    stream
        .current_turn_mut()
        .add_chat_response(response)
        .build()
        .unwrap();
}

/// A stream of one turn per entry in `responses`, each ending in that response.
fn stream_of(responses: Vec<ChatResponse>) -> ConversationStream {
    let mut stream = ConversationStream::new_test();
    for response in responses {
        add_turn(&mut stream, response);
    }
    stream
}

/// A schema asking for a JSON object.
fn object_schema() -> Map<String, Value> {
    Map::from_iter([("type".to_owned(), json!("object"))])
}

#[test]
fn a_structured_query_replies_with_the_last_turns_data() {
    let mut events = stream_of(vec![ChatResponse::structured(json!({"turn": 1}))]);
    let before = last_request_id(&events);
    add_turn(&mut events, ChatResponse::structured(json!({"turn": 2})));

    assert_eq!(
        completed(
            Some("q1".to_owned()),
            "123".to_owned(),
            &events,
            before.as_ref(),
            Some(&object_schema()),
        ),
        HostToPlugin::QueryComplete(QueryCompleteResponse {
            id: Some("q1".to_owned()),
            conversation: "123".to_owned(),
            data: Some(json!({"turn": 2})),
        })
    );
}

/// The turn that just ran answered in prose, so an earlier turn's data must not
/// be handed back in its place.
#[test]
fn a_structured_query_whose_turn_produced_no_data_fails() {
    let mut events = stream_of(vec![ChatResponse::structured(json!({"turn": 1}))]);
    let before = last_request_id(&events);
    add_turn(&mut events, ChatResponse::message("I would rather not."));

    assert_eq!(
        completed(
            Some("q1".to_owned()),
            "123".to_owned(),
            &events,
            before.as_ref(),
            Some(&object_schema()),
        ),
        HostToPlugin::Error(ErrorResponse {
            id: Some("q1".to_owned()),
            request: Some("query".to_owned()),
            message: "conversation 123: no structured data in the assistant's response".to_owned(),
        })
    );
}

/// A client stop that lands while MCP servers start or the model is looked up
/// ends the turn before its request is appended, leaving the previous turn's
/// data last in the conversation.
#[test]
fn a_structured_query_stopped_before_its_turn_started_fails() {
    let events = stream_of(vec![ChatResponse::structured(json!({"turn": 1}))]);
    let before = last_request_id(&events);

    assert_eq!(
        completed(
            Some("q1".to_owned()),
            "123".to_owned(),
            &events,
            before.as_ref(),
            Some(&object_schema()),
        ),
        HostToPlugin::Error(ErrorResponse {
            id: Some("q1".to_owned()),
            request: Some("query".to_owned()),
            message: "conversation 123: the turn was stopped before it started".to_owned(),
        })
    );
}

/// A response cut off at the output token limit mid-object is recorded as its
/// raw text, which an object schema rules out as an answer.
#[test]
fn a_structured_query_whose_response_was_cut_short_fails() {
    // The query created the conversation, so there was no request before it.
    let events = stream_of(vec![ChatResponse::structured(json!(r#"{"summary": "sho"#))]);

    assert_eq!(
        completed(
            Some("q1".to_owned()),
            "123".to_owned(),
            &events,
            None,
            Some(&object_schema()),
        ),
        HostToPlugin::Error(ErrorResponse {
            id: Some("q1".to_owned()),
            request: Some("query".to_owned()),
            message: "conversation 123: the assistant's structured response is not valid JSON; it \
                      may have been cut off at the output token limit"
                .to_owned(),
        })
    );
}

#[test]
fn a_query_without_a_schema_replies_without_data() {
    let events = stream_of(vec![ChatResponse::structured(json!({"turn": 1}))]);

    assert_eq!(
        completed(None, "123".to_owned(), &events, None, None),
        HostToPlugin::QueryComplete(QueryCompleteResponse {
            id: None,
            conversation: "123".to_owned(),
            data: None,
        })
    );
}

/// `jp <plugin> -h` without a terminal: a binary nobody approved is refused
/// before it runs, and once approved it answers.
///
/// The script writes a marker whenever it runs, so its absence proves it was
/// never spawned, `describe` included.
///
/// Unix only: on Windows a plugin is a `jp-*.exe`, which a test cannot write as
/// a script.
#[cfg(unix)]
#[test]
#[serial(env_vars)]
fn plugin_help_does_not_run_an_unapproved_binary() {
    use std::os::unix::fs::PermissionsExt as _;

    let tmp = tempdir().unwrap();
    let bin = tmp.path().join("bin");
    let data = tmp.path().join("data");
    let marker = tmp.path().join("ran");
    fs::create_dir_all(&bin).unwrap();

    let script = bin.join("jp-titles");
    fs::write(
        &script,
        format!(
            r#"#!/bin/sh
# jp-plugin/v1 {{"protocol":1,"description":"Titles","command":["titles"]}}
: > {marker}
read -r msg
echo '{{"type":"describe","protocol":1,"name":"titles","version":"0.1.0","description":"Titles","command":["titles"],"help":"Usage: jp titles"}}'
"#
        ),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

    let _path = EnvVarGuard::set("PATH", bin.as_str());
    let _data = EnvVarGuard::set("JP_USER_DATA_DIR", data.as_str());
    let _offline = EnvVarGuard::set("JP_NO_PLUGIN_DOWNLOAD", "1");

    let workspace = Workspace::in_memory(tmp.path().join("workspace"));
    let (printer, _out, _err) = Printer::memory(OutputFormat::Text);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut ctx = Ctx::new(
        crate::bootstrap::ExecutionContext::for_workspace(&workspace),
        workspace,
        None,
        tokio::runtime::Runtime::new().unwrap(),
        Globals {
            no_interactive: true,
            ..Globals::default()
        },
        AppConfig::new_test(),
        None,
        printer,
    );
    let args = ["titles".to_owned(), "-h".to_owned()];

    let error = runtime.block_on(run_external(&args, &mut ctx)).unwrap_err();
    assert_eq!(
        error.message.as_deref(),
        Some(
            format!(
                "plugin `titles` at {script} is not approved. Approve it with `jp plugin approve \
                 {script}`, or set plugins.command.titles.run = \"allow\" in config."
            )
            .as_str()
        )
    );
    assert!(!marker.exists(), "the unapproved binary was never run");

    ApprovalStore::load()
        .record("titles", ApprovedPlugin {
            path: script.canonicalize_utf8().unwrap(),
            sha256: registry::sha256_file(&script).unwrap(),
            approved_at: Utc::now(),
            installed: false,
            manifest: None,
        })
        .unwrap();

    runtime.block_on(run_external(&args, &mut ctx)).unwrap();
    assert!(marker.exists(), "the approved binary answered describe");
}

/// A plugin that starts a worker, answers `init`, sends `exit`, and exits on
/// its own: the worker is stopped with it, not left running once `jp` returns.
///
/// Unix only: on Windows a plugin is a `jp-*.exe`, which a test cannot write as
/// a script.
/// `process_tree::tests` covers the tree on every platform.
#[cfg(unix)]
#[test]
fn a_worker_dies_with_a_plugin_that_exits_on_its_own() {
    use std::os::unix::fs::PermissionsExt as _;

    let tmp = tempdir().unwrap();
    let script = tmp.path().join("jp-server");
    let pid_file = tmp.path().join("worker.pid");
    fs::write(
        &script,
        format!(
            r#"#!/bin/sh
sleep 600 &
echo $! > {pid_file}
read -r msg
echo '{{"type":"ready","protocol":1}}'
echo '{{"type":"exit","code":0}}'
"#
        ),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

    let root = tmp.path().join("workspace");
    let backend = Arc::new(FsStorageBackend::new(&root.join(".jp")).unwrap());
    let workspace = Workspace::in_memory(&root);
    let (printer, _out, _err) = Printer::memory(OutputFormat::Text);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut ctx = Ctx::new(
        crate::bootstrap::ExecutionContext::for_workspace(&workspace),
        workspace,
        Some(backend),
        tokio::runtime::Runtime::new().unwrap(),
        Globals::default(),
        AppConfig::new_test(),
        None,
        printer,
    );

    runtime
        .block_on(run_plugin("server", &script, &[], &mut ctx))
        .unwrap();

    let worker: u32 = fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();

    // The killed worker is an orphan, reaped by init a moment later.
    let deadline = Instant::now() + Duration::from_secs(10);
    while is_process_alive(worker) {
        assert!(Instant::now() < deadline, "the worker outlived its plugin");
        thread::sleep(Duration::from_millis(20));
    }
}

/// A plugin that closes its stdin, starts a worker, and exits without reading
/// `init`: the host's write fails, and the worker is stopped all the same.
///
/// The argument is larger than any pipe buffer, so the write cannot complete
/// into the buffer before the plugin exits, and fails with a broken pipe every
/// time.
///
/// Unix only: on Windows a plugin is a `jp-*.exe`, which a test cannot write as
/// a script.
#[cfg(unix)]
#[test]
fn a_worker_dies_with_a_plugin_that_never_reads_init() {
    use std::os::unix::fs::PermissionsExt as _;

    let tmp = tempdir().unwrap();
    let script = tmp.path().join("jp-launcher");
    let pid_file = tmp.path().join("worker.pid");

    // Stdin is closed before the worker starts, so the worker does not inherit
    // it and keep the host's write blocked.
    fs::write(
        &script,
        format!("#!/bin/sh\nexec 0<&-\nsleep 600 &\necho $! > {pid_file}\n"),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

    let root = tmp.path().join("workspace");
    let backend = Arc::new(FsStorageBackend::new(&root.join(".jp")).unwrap());
    let workspace = Workspace::in_memory(&root);
    let (printer, _out, _err) = Printer::memory(OutputFormat::Text);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut ctx = Ctx::new(
        crate::bootstrap::ExecutionContext::for_workspace(&workspace),
        workspace,
        Some(backend),
        tokio::runtime::Runtime::new().unwrap(),
        Globals::default(),
        AppConfig::new_test(),
        None,
        printer,
    );

    let args = ["x".repeat(1024 * 1024)];
    let error = runtime
        .block_on(run_plugin("launcher", &script, &args, &mut ctx))
        .unwrap_err();

    // Prefix only: the rest is the operating system's wording for a broken
    // pipe.
    let message = error.message.unwrap_or_default();
    assert!(message.starts_with("failed to send init: "), "{message}");

    let worker: u32 = fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();

    // The killed worker is an orphan, reaped by init a moment later.
    let deadline = Instant::now() + Duration::from_secs(10);
    while is_process_alive(worker) {
        assert!(Instant::now() < deadline, "the worker outlived its plugin");
        thread::sleep(Duration::from_millis(20));
    }
}

/// A plugin that ignores `Shutdown` is killed once
/// `plugins.shutdown_timeout_secs` has passed, not after a fixed grace period.
///
/// With the timeout at zero the kill follows the request at once; at the
/// default of five seconds the run would take at least that long to end.
///
/// Unix only: on Windows a plugin is a `jp-*.exe`, which a test cannot write as
/// a script.
#[cfg(unix)]
#[test]
fn a_plugin_ignoring_shutdown_is_killed_after_the_configured_grace() {
    use std::os::unix::fs::PermissionsExt as _;

    let tmp = tempdir().unwrap();
    let script = tmp.path().join("jp-stubborn");
    let started = tmp.path().join("started");

    // Never reads stdin again, so the `Shutdown` it is sent goes unanswered.
    fs::write(
        &script,
        format!(
            "#!/bin/sh\nread -r msg\necho '{{\"type\":\"ready\",\"protocol\":1}}'\n: > \
             {started}\nsleep 600\n"
        ),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

    let mut config = AppConfig::new_test();
    config.plugins.shutdown_timeout_secs = 0;

    let root = tmp.path().join("workspace");
    let backend = Arc::new(FsStorageBackend::new(&root.join(".jp")).unwrap());
    let workspace = Workspace::in_memory(&root);
    let (printer, _out, _err) = Printer::memory(OutputFormat::Text);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut ctx = Ctx::new(
        crate::bootstrap::ExecutionContext::for_workspace(&workspace),
        workspace,
        Some(backend),
        tokio::runtime::Runtime::new().unwrap(),
        Globals::default(),
        config,
        None,
        printer,
    );

    // Requested once the plugin is running, the way a SIGTERM would be.
    let shutdown = ctx.signals.shutdown_token();
    let requested = Arc::new(Mutex::new(None));
    {
        let requested = Arc::clone(&requested);
        thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !started.exists() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(20));
            }
            *requested.lock().unwrap() = Some(Instant::now());
            shutdown.cancel();
        });
    }

    runtime
        .block_on(run_plugin("stubborn", &script, &[], &mut ctx))
        .unwrap();

    let requested = requested.lock().unwrap().expect("shutdown was requested");
    let took = requested.elapsed();
    assert!(
        took < Duration::from_secs(3),
        "killed {took:?} after shutdown was requested"
    );
}

#[test]
fn a_describe_answer_is_compared_with_the_manifest_field_by_field() {
    let manifest = Manifest {
        protocol: 1,
        description: "Titles".to_owned(),
        command: vec!["titles".to_owned()],
    };

    assert!(manifest_differences(&manifest, &manifest).is_empty());
    assert_eq!(
        manifest_differences(&manifest, &Manifest {
            protocol: 2,
            command: vec!["other".to_owned()],
            ..manifest.clone()
        }),
        ["protocol", "command"]
    );
}

/// A script plugin whose `describe` answer claims another command than its
/// manifest: the host refuses the answer rather than choosing between them.
///
/// Unix only: on Windows a plugin is a `jp-*.exe`, which a test cannot write as
/// a script.
/// `a_describe_answer_is_compared_with_the_manifest_field_by_field` covers the
/// comparison on every platform.
#[cfg(unix)]
#[test]
fn a_describe_answer_that_disagrees_with_the_manifest_is_refused() {
    use crate::cmd::plugin::discovery::{Location, ManifestState, read_manifest};

    let tmp = tempdir().unwrap();
    let path = tmp.path().join("jp-titles");
    fs::write(
        &path,
        r#"#!/bin/sh
# jp-plugin/v1 {"protocol":1,"description":"Titles","command":["titles"]}
read -r msg
echo '{"type":"describe","protocol":1,"name":"titles","version":"0.1.0","description":"Titles","command":["other"],"help":"Usage"}'
"#,
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let plugin = LocalPlugin {
        name: "titles".to_owned(),
        manifest: read_manifest(&path),
        path: path.clone(),
        location: Location::Path,
    };
    assert!(matches!(plugin.manifest, ManifestState::Valid(_)));

    let error = describe(&plugin, &CancellationToken::new()).unwrap_err();

    assert_eq!(
        error.message.as_deref(),
        Some(
            format!(
                "the plugin `titles` at {path} describes itself differently from its manifest \
                 (command), so it does not run"
            )
            .as_str()
        )
    );
}

/// A plugin that starts a worker, answers `describe`, and then keeps running:
/// asking it to describe itself returns, and takes the worker with it.
///
/// Unix only: on Windows a plugin is a `jp-*.exe`, which a test cannot write as
/// a script.
/// `process_tree::tests` covers the tree on every platform.
#[cfg(unix)]
#[test]
fn describing_a_plugin_stops_everything_it_started() {
    use std::{os::unix::fs::PermissionsExt as _, sync::mpsc};

    let tmp = tempdir().unwrap();
    let path = tmp.path().join("jp-titles");
    let pid_file = tmp.path().join("worker.pid");
    fs::write(
        &path,
        format!(
            r#"#!/bin/sh
sleep 60 &
echo $! > {pid_file}
read -r msg
echo '{{"type":"describe","protocol":1,"name":"titles","version":"0.1.0","description":"Titles","command":["titles"],"help":"Usage"}}'
sleep 60
"#
        ),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();

    // On a thread, so a describe that waits on the plugin fails the test
    // instead of hanging it.
    let (tx, rx) = mpsc::channel();
    let binary = path.clone();
    thread::spawn(move || drop(tx.send(describe_plugin(&binary, &CancellationToken::new()))));

    let answer = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("describe returned rather than waiting on the plugin");
    assert_eq!(answer.map(|a| a.name).as_deref(), Some("titles"));

    let worker: u32 = fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();

    // The killed worker is an orphan, reaped by init a moment later.
    let deadline = Instant::now() + Duration::from_secs(10);
    while is_process_alive(worker) {
        assert!(Instant::now() < deadline, "the worker outlived describe");
        thread::sleep(Duration::from_millis(20));
    }
}

/// A plugin that starts a worker and never answers: cancelling the describe, as
/// Ctrl-C does, stops it and the worker, and reports the call as interrupted.
///
/// Unix only: on Windows a plugin is a `jp-*.exe`, which a test cannot write as
/// a script.
#[cfg(unix)]
#[test]
fn a_cancelled_describe_stops_everything_the_plugin_started() {
    use std::{os::unix::fs::PermissionsExt as _, sync::mpsc};

    use crate::cmd::plugin::discovery::{Location, ManifestState};

    let tmp = tempdir().unwrap();
    let path = tmp.path().join("jp-slow");
    let pid_file = tmp.path().join("worker.pid");
    fs::write(
        &path,
        format!("#!/bin/sh\nsleep 60 &\necho $! > {pid_file}\nsleep 60\n"),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();

    let plugin = LocalPlugin {
        name: "slow".to_owned(),
        path,
        location: Location::Path,
        manifest: ManifestState::Missing,
    };

    let cancel = CancellationToken::new();
    let (tx, rx) = mpsc::channel();
    {
        let cancel = cancel.clone();
        thread::spawn(move || {
            let result = describe(&plugin, &cancel).map_err(|e| (e.code.get(), e.message));
            drop(tx.send(result));
        });
    }

    // Cancelled once the plugin is running, and before it could answer.
    let deadline = Instant::now() + Duration::from_secs(10);
    let worker: u32 = loop {
        if let Ok(pid) = fs::read_to_string(&pid_file)
            && let Ok(pid) = pid.trim().parse()
        {
            break pid;
        }
        assert!(Instant::now() < deadline, "the plugin never started");
        thread::sleep(Duration::from_millis(20));
    };
    cancel.cancel();

    let result = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("describe returned rather than waiting on the plugin");
    assert_eq!(result.unwrap_err(), (130, Some("Interrupted".to_owned())));

    let deadline = Instant::now() + Duration::from_secs(10);
    while is_process_alive(worker) {
        assert!(Instant::now() < deadline, "the worker outlived the cancel");
        thread::sleep(Duration::from_millis(20));
    }
}

/// A registry with the official `serve` group and nothing under it.
fn serve_group() -> Registry {
    Registry {
        version: 1,
        plugins: [("serve".to_owned(), RegistryPlugin {
            id: "serve".to_owned(),
            description: "JP server components".to_owned(),
            official: true,
            repository: None,
            kind: PluginKind::CommandGroup { suggests: vec![] },
        })]
        .into_iter()
        .collect(),
    }
}

fn needs(line: &str, registry: &Registry) -> bool {
    let args: Vec<String> = line.split(' ').map(ToOwned::to_owned).collect();
    let route = routing::route(&args, &[], Some(registry));
    needs_registry(&args, &route)
}

/// A command published under a group after the cache was written routes to the
/// group, which knows nothing of it: only a fresh registry can.
#[test]
fn an_unknown_command_under_a_group_needs_the_registry() {
    let registry = serve_group();

    assert!(needs("serve http-api", &registry));
    assert!(needs("serve http-api --port 1", &registry));
}

/// The group itself, and its help, are answered from the cache.
#[test]
fn a_group_or_its_help_does_not_need_the_registry() {
    let registry = serve_group();

    assert!(!needs("serve", &registry));
    assert!(!needs("serve -h", &registry));
    assert!(!needs("serve --help", &registry));
}

#[test]
fn a_command_nothing_claims_needs_the_registry() {
    assert!(needs("frobnicate", &serve_group()));
}

/// Running a plugin installed on this machine never reaches the network.
#[test]
fn a_command_an_installed_plugin_claims_does_not_need_the_registry() {
    use crate::cmd::plugin::discovery::{Location, ManifestState};

    let local = [LocalPlugin {
        name: "webui".to_owned(),
        path: "/bin/jp-webui".into(),
        location: Location::Path,
        manifest: ManifestState::Valid(Manifest {
            protocol: 1,
            description: "Web UI".to_owned(),
            command: vec!["serve".to_owned(), "web".to_owned()],
        }),
    }];
    let args = vec!["serve".to_owned(), "web".to_owned()];
    let registry = serve_group();

    let route = routing::route(&args, &local, Some(&registry));

    assert!(!needs_registry(&args, &route));
}
