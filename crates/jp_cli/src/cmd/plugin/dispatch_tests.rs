use camino_tempfile::{Utf8TempDir, tempdir};
use jp_conversation::{Conversation, ConversationId};
use jp_plugin::message::{ExitMessage, ReadyMessage};
use jp_storage::backend::FsStorageBackend;
use serde_json::json;

use super::*;

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
    let tmp = tempdir().unwrap();
    let fs = Arc::new(FsStorageBackend::new(&tmp.path().join(".jp")).unwrap());

    let id = ConversationId::try_from(
        chrono::DateTime::<chrono::Utc>::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000),
    )
    .unwrap();
    fs.write_test_conversation(&id, &Conversation::default());

    let mut workspace = Workspace::in_memory(tmp.path()).with_backend(fs);
    workspace.load_conversation_index();

    (workspace, id, tmp)
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

    // No terminal in a test, so a compose request would be declined rather
    // than prompting; this exchange never asks for one.
    let printer = jp_printer::Printer::sink();
    let prompts = jp_inquire::prompt::TerminalPromptBackend;
    let composer = Composer {
        printer: &printer,
        prompts: &prompts,
        editor: None,
        edit_mode: jp_inquire::ReplyEditMode::default(),
        is_tty: false,
    };

    message_loop(
        reader,
        &sink,
        &mut ws,
        &config,
        &shutdown_sent,
        &composer,
        None,
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
        is_tty: false,
    };

    let error = message_loop(
        reader,
        &sink,
        &mut ws,
        &config,
        &shutdown_sent,
        &composer,
        None,
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
