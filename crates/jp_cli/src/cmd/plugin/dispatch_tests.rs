use camino_tempfile::{Utf8TempDir, tempdir};
use jp_conversation::{Conversation, ConversationId};
use jp_plugin::message::{ExitMessage, ReadyMessage};
use jp_storage::backend::FsStorageBackend;
use serde_json::json;

use super::*;

/// A workspace no request in these tests reaches into, so it needs no storage.
fn bare_workspace() -> Workspace {
    Workspace::new("/tmp/jp-test-plugin")
}

/// A router with no signal source, for requests that never reach one.
///
/// Must be called inside a tokio runtime, which is why the tests using it are
/// `#[tokio::test]` despite `handle_request` being synchronous.
fn router() -> SignalRouter {
    crate::signals::testing::detached_router()
}

/// How a conversation is spelled on the wire, matching `list_conversations`.
fn wire_id(id: ConversationId) -> String {
    id.to_string()
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

    let mut workspace = Workspace::new(tmp.path()).with_backend(fs.clone());
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

/// A long-running host sees what another process wrote after it started.
///
/// The host loads the index once at startup.
/// Without re-reading it, a plugin asking for the conversation list is served
/// that snapshot for the life of the process, so a conversation started in a
/// terminal never appears.
#[tokio::test]
async fn a_conversation_written_after_startup_is_listed() {
    let (mut ws, first, fs, tmp) = workspace_with_drafts();
    let mut sink: Vec<u8> = Vec::new();

    // The host's view, taken at startup.
    ws.load_conversation_index();
    assert_eq!(ws.conversations().count(), 1);

    // Another process writes a second conversation. Same store, its own handle,
    // which is what a `jp query` in a terminal amounts to.
    let second = ConversationId::try_from(
        chrono::DateTime::<chrono::Utc>::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_001),
    )
    .unwrap();
    fs.write_test_conversation(&second, &Conversation::default());

    let response = handle_request(
        PluginToHost::ListConversations(jp_plugin::message::OptionalId { id: None }),
        &mut sink,
        &mut ws,
        &json!({}),
        None,
        None,
        &AppConfig::new_test(),
        &router(),
    )
    .unwrap();
    assert_eq!(response, Flow::Continue);

    let listed: Vec<String> = ws.conversations().map(|(id, _)| id.to_string()).collect();

    assert!(
        listed.contains(&second.to_string()),
        "a conversation written after startup must be listed: {listed:?}"
    );
    assert!(
        listed.contains(&first.to_string()),
        "and the one from startup is still there: {listed:?}"
    );

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
            &router(),
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
            &router(),
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
        &router(),
    )
    .expect_err("a non-zero exit is an error");

    assert!(error.to_string().contains("no such ticket"), "{error}");
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
        &router(),
    )
    .expect_err("a plugin needing a newer protocol must be refused");

    assert!(
        error.to_string().contains("Reinstall the two together"),
        "{error}"
    );
}

#[test]
fn find_plugin_binary_nonexistent() {
    let result = find_plugin_binary(&["__jp_test_nonexistent_plugin_42__"]);
    assert!(result.is_none());
}
