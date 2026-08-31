use jp_plugin::message::{ExitMessage, ReadyMessage};
use serde_json::json;

use super::*;

/// A workspace no request in these tests reaches into, so it needs no storage.
fn bare_workspace() -> Workspace {
    Workspace::in_memory("/tmp/jp-test-plugin")
}

#[test]
fn a_ready_carries_on_and_a_clean_exit_stops() {
    let ws = bare_workspace();
    let config = json!({});
    let mut sink: Vec<u8> = Vec::new();

    assert_eq!(
        handle_request(
            PluginToHost::Ready(ReadyMessage { protocol: 1 }),
            &mut sink,
            &ws,
            &config,
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
            &ws,
            &config,
        )
        .unwrap(),
        Flow::Stop
    );
}

/// A non-zero exit is the plugin's failure, so it surfaces as one rather than
/// ending the run quietly.
#[test]
fn a_failing_exit_carries_its_code_and_reason() {
    let ws = bare_workspace();
    let mut sink: Vec<u8> = Vec::new();

    let error = handle_request(
        PluginToHost::Exit(ExitMessage {
            code: 3,
            reason: Some("no such ticket".to_owned()),
        }),
        &mut sink,
        &ws,
        &json!({}),
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
    let ws = jp_workspace::Workspace::in_memory("/tmp/jp-test-plugin");

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

    message_loop(reader, &sink, &ws, &config, &shutdown_sent, &composer).unwrap();
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
    let ws = jp_workspace::Workspace::in_memory("/tmp/jp-test-plugin");

    let printer = jp_printer::Printer::sink();
    let prompts = jp_inquire::prompt::TerminalPromptBackend;
    let composer = Composer {
        printer: &printer,
        prompts: &prompts,
        editor: None,
        edit_mode: jp_inquire::ReplyEditMode::default(),
        is_tty: false,
    };

    let error = message_loop(reader, &sink, &ws, &config, &shutdown_sent, &composer)
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
