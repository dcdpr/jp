use camino_tempfile::{Utf8TempDir, tempdir};
use jp_conversation::{Conversation, ConversationId};
use jp_plugin::message::{
    ExitMessage, InterruptRequest, OptionalId, ReadEventsRequest, ReadyMessage,
};
use jp_storage::backend::{FsStorageBackend, PersistBackend as _};
use relative_path::RelativePathBuf;
use serde_json::json;
use serial_test::serial;

use super::*;
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

/// A plugin that ignores `Shutdown` is killed rather than waited on forever.
///
/// The host holds the only handles to the plugin's stdin — this scope and the
/// shutdown thread — so a plugin blocked on a read never sees EOF and never
/// exits on its own.
/// Waiting on one is a wait with no end, and it takes the error that caused it
/// down with it.
///
/// The child here is `sleep`, which is exactly that plugin: it reads nothing
/// and exits on nothing short of a signal.
#[cfg(unix)]
#[test]
fn stop_plugin_kills_a_plugin_that_will_not_go() {
    use std::{process::Stdio, sync::mpsc};

    let mut child = std::process::Command::new("sleep")
        .arg("60")
        .stdin(Stdio::piped())
        .spawn()
        .expect("sleep is available");

    let id = child.id();
    let stdin = Mutex::new(child.stdin.take().expect("stdin piped"));
    let sent = AtomicBool::new(false);

    // On its own thread with a deadline: if `stop_plugin` ever goes back to
    // waiting indefinitely, this fails rather than hanging the suite — which is
    // the failure mode being guarded against.
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        stop_plugin(&stdin, &sent, id, Duration::from_millis(200));
        let _ = tx.send(());
    });

    rx.recv_timeout(Duration::from_secs(10))
        .expect("stop_plugin returned rather than waiting forever");

    assert!(
        child.wait().is_ok(),
        "the child is reaped, so it is no longer running"
    );
    assert!(
        !is_process_alive(id),
        "a plugin that ignored the request is gone"
    );
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

#[test]
fn find_plugin_binary_nonexistent() {
    let result = find_plugin_binary(&["__jp_test_nonexistent_plugin_42__"]);
    assert!(result.is_none());
}
