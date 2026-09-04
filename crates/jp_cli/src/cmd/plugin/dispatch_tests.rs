use camino_tempfile::{Utf8TempDir, tempdir};
use jp_conversation::{Conversation, ConversationId};
use jp_plugin::message::{ExitMessage, ReadyMessage};
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

#[test]
fn a_ready_carries_on_and_a_clean_exit_stops() {
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
#[test]
fn a_failing_exit_carries_its_code_and_reason() {
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

#[test]
fn message_loop_ready_then_exit() {
    use std::io::{BufReader, Cursor};

    let plugin_output = [r#"{"type":"ready"}"#, r#"{"type":"exit","code":0}"#].join("\n");

    let reader = BufReader::new(Cursor::new(plugin_output));
    let sink: Mutex<Vec<u8>> = Mutex::new(Vec::new());
    let config = json!({});
    let shutdown_sent = AtomicBool::new(false);

    // We can't easily construct a Workspace for a unit test without a temp dir,
    // but this test only exercises ready + exit (no workspace queries). We
    // construct a minimal in-memory workspace.
    let mut ws = jp_workspace::Workspace::in_memory("/tmp/jp-test-plugin");

    // No user to ask in a test, so a compose request would be declined rather
    // than prompting; this exchange never asks for one.
    let printer = jp_printer::Printer::sink();
    let prompts = jp_inquire::prompt::TerminalPromptBackend;
    let composer = Composer {
        printer: &printer,
        prompts: &prompts,
        editor: None,
        edit_mode: jp_inquire::ReplyEditMode::default(),
        interactive: false,
    };

    message_loop(
        reader,
        &sink,
        &mut ws,
        &config,
        &shutdown_sent,
        &composer,
        None,
        None,
        &AppConfig::new_test(),
    )
    .unwrap();
}

/// A plugin needing a newer protocol than this host is refused on its `ready`.
///
/// The `exit 0` behind it would end the loop successfully, so an `Err` here can
/// only come from the version check.
#[test]
fn message_loop_refuses_a_plugin_needing_a_newer_protocol() {
    use std::io::{BufReader, Cursor};

    let plugin_output = [
        &format!(r#"{{"type":"ready","protocol":{}}}"#, PROTOCOL_VERSION + 1),
        r#"{"type":"exit","code":0}"#,
    ]
    .join("\n");

    let reader = BufReader::new(Cursor::new(plugin_output));
    let sink: Mutex<Vec<u8>> = Mutex::new(Vec::new());
    let config = json!({});
    let shutdown_sent = AtomicBool::new(false);
    let mut ws = jp_workspace::Workspace::in_memory("/tmp/jp-test-plugin");

    let printer = jp_printer::Printer::sink();
    let prompts = jp_inquire::prompt::TerminalPromptBackend;
    let composer = Composer {
        printer: &printer,
        prompts: &prompts,
        editor: None,
        edit_mode: jp_inquire::ReplyEditMode::default(),
        interactive: false,
    };

    let error = message_loop(
        reader,
        &sink,
        &mut ws,
        &config,
        &shutdown_sent,
        &composer,
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
