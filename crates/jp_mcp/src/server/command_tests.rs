use camino_tempfile::tempdir;
use serde_json::json;

use super::*;

/// What a plugin printing `lines`, one protocol message each, printed.
fn printed(lines: &[Value]) -> String {
    lines.iter().map(|line| line.to_string() + "\n").collect()
}

fn outcome(content: &str) -> Value {
    json!({"type": "tool_outcome", "outcome": {"type": "success", "content": content}})
}

fn exit(code: u8) -> Value {
    json!({"type": "exit", "code": code})
}

#[test]
fn the_outcome_is_read_back() {
    let stdout = printed(&[
        json!({"type": "ready", "protocol": 2}),
        outcome("Created T-0abc123"),
        exit(0),
    ]);

    assert_eq!(
        parse_plugin_output(&stdout),
        Ok(json!({"type": "success", "content": "Created T-0abc123"}))
    );
}

/// Stdin carries only `init`, so a request, a print, or a log the plugin sends
/// gets no answer and does not stop the outcome arriving.
#[test]
fn other_messages_are_ignored() {
    let stdout = printed(&[
        json!({"type": "ready", "protocol": 2}),
        json!({"type": "list_conversations", "id": "3"}),
        json!({"type": "print", "text": "working"}),
        json!({"type": "log", "level": "warn", "message": "slow disk"}),
        outcome("ok"),
        exit(0),
    ]);

    assert_eq!(
        parse_plugin_output(&stdout),
        Ok(json!({"type": "success", "content": "ok"}))
    );
}

#[test]
fn a_failing_exit_reports_its_reason() {
    let stdout = printed(&[json!({"type": "exit", "code": 1, "reason": "No ticket T-0abc123."})]);

    assert_eq!(
        parse_plugin_output(&stdout),
        Err("No ticket T-0abc123.".to_owned())
    );
}

#[test]
fn a_failing_exit_without_a_reason_reports_its_code() {
    assert_eq!(
        parse_plugin_output(&printed(&[exit(3)])),
        Err("exited with code 3".to_owned())
    );
}

/// What a plugin built before protocol 12 does: it ignores `tool`, finds no
/// arguments to run, and exits without an outcome.
#[test]
fn an_exit_without_an_outcome_is_a_failure() {
    assert_eq!(
        parse_plugin_output(&printed(&[exit(0)])),
        Err("exited without answering the tool call; it may predate `jp` protocol 12".to_owned())
    );
}

/// A plugin that answers and then dies before `exit`, a panic during teardown
/// or a failed write, did not finish the call.
/// Its outcome is not reported as the call's result.
#[test]
fn an_outcome_without_an_exit_is_a_failure() {
    let stdout = printed(&[json!({"type": "ready", "protocol": 12}), outcome("ok")]);

    assert_eq!(
        parse_plugin_output(&stdout),
        Err("answered the tool call, then exited without sending `exit`".to_owned())
    );
}

#[test]
fn no_output_at_all_is_a_failure() {
    assert_eq!(
        parse_plugin_output(""),
        Err("exited without answering the tool call; it may predate `jp` protocol 12".to_owned())
    );
}

#[test]
fn a_plugin_needing_a_newer_protocol_is_refused() {
    let stdout = printed(&[
        json!({"type": "ready", "protocol": PROTOCOL_VERSION + 1}),
        outcome("ok"),
    ]);

    assert_eq!(
        parse_plugin_output(&stdout),
        Err(format!(
            "it needs `jp` protocol {}, and this `jp` speaks {PROTOCOL_VERSION}",
            PROTOCOL_VERSION + 1
        ))
    );
}

#[test]
fn a_line_that_is_not_a_message_is_a_failure() {
    let error = parse_plugin_output("not json\n").unwrap_err();

    assert!(
        error.starts_with("sent a line that is not a protocol message: "),
        "got: {error}"
    );
}

#[test]
fn verify_accepts_the_admitted_contents_and_refuses_changed_ones() {
    let dir = tempdir().unwrap();
    let binary = dir.path().join("jp-ticket");
    fs::write(&binary, "v1").unwrap();
    let plugins = CommandPlugins::default().with("ticket", AdmittedPlugin {
        sha256: sha256_file(&binary).unwrap(),
        binary: binary.clone(),
        options: Map::new(),
    });

    assert_eq!(plugins.verify("ticket").map(|p| &p.binary), Ok(&binary));

    fs::write(&binary, "v2").unwrap();
    assert_eq!(
        plugins.verify("ticket"),
        Err(format!(
            "{binary} changed since it was admitted at the start of this turn"
        ))
    );
    assert_eq!(
        plugins.verify("metrics"),
        Err("it was not admitted for this turn".to_owned())
    );
}

/// The digest the registry and the approval store use, so a hash recorded by
/// either reads the same here.
#[test]
fn sha256_is_lowercase_hex() {
    let dir = tempdir().unwrap();
    let file = dir.path().join("f");
    fs::write(&file, "abc").unwrap();

    assert_eq!(
        sha256_file(&file).unwrap(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}
