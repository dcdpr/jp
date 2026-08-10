use serde_json::json;

use super::*;

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
    let ws = jp_workspace::Workspace::new("/tmp/jp-test-plugin");

    // No terminal in a test, so a compose request would be declined rather
    // than prompting; this exchange never asks for one.
    let printer = jp_printer::Printer::sink();
    let composer = Composer {
        printer: &printer,
        editor: None,
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
    let ws = jp_workspace::Workspace::new("/tmp/jp-test-plugin");

    let printer = jp_printer::Printer::sink();
    let composer = Composer {
        printer: &printer,
        editor: None,
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
