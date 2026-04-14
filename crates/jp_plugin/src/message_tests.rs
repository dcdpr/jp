use pretty_assertions::assert_eq;
use serde_json::{Map, from_str, json};

use super::*;

#[test]
fn host_init_roundtrip() {
    let msg = HostToPlugin::Init(InitMessage {
        version: 1,
        workspace: WorkspaceInfo {
            root: "/project".into(),
            storage: "/project/.jp".into(),
            id: "abc12".to_owned(),
        },
        paths: PathsInfo {
            user_data: Some("/home/user/.local/share/jp".into()),
            user_config: Some("/home/user/.config/jp".into()),
            user_workspace: None,
        },
        config: json!({"assistant": {"name": "JP"}}),
        options: Map::from_iter([("port".to_owned(), json!(8080))]),
        args: vec!["--web".to_owned()],
        log_level: 0,
    });

    let json = serde_json::to_string(&msg).unwrap();
    let parsed: HostToPlugin = from_str(&json).unwrap();
    assert_eq!(msg, parsed);
}

#[test]
fn host_init_deserializes_from_json() {
    let json = r#"{"type":"init","version":1,"workspace":{"root":"/p","storage":"/p/.jp","id":"x"},"config":{},"args":[]}"#;
    let msg: HostToPlugin = from_str(json).unwrap();
    assert!(matches!(msg, HostToPlugin::Init(_)));
}

#[test]
fn host_init_paths_default_when_absent() {
    // Older hosts may not send the `paths` field. It should default gracefully.
    let json = r#"{"type":"init","version":1,"workspace":{"root":"/p","storage":"/p/.jp","id":"x"},"config":{}}"#;
    let msg: HostToPlugin = from_str(json).unwrap();
    if let HostToPlugin::Init(init) = msg {
        assert_eq!(init.paths, PathsInfo::default());
    } else {
        panic!("expected Init");
    }
}

#[test]
fn paths_info_omits_none_fields() {
    let paths = PathsInfo {
        user_data: Some("/data".into()),
        user_config: None,
        user_workspace: None,
    };
    let json = serde_json::to_string(&paths).unwrap();
    assert!(json.contains("user_data"));
    assert!(!json.contains("user_config"));
    assert!(!json.contains("user_workspace"));
}

#[test]
fn plugin_ready_roundtrip() {
    let msg = PluginToHost::Ready;
    let json = serde_json::to_string(&msg).unwrap();
    assert_eq!(json, r#"{"type":"ready"}"#);

    let parsed: PluginToHost = from_str(&json).unwrap();
    assert_eq!(msg, parsed);
}

#[test]
fn plugin_list_conversations_roundtrip() {
    let msg = PluginToHost::ListConversations(OptionalId { id: None });
    let json = serde_json::to_string(&msg).unwrap();
    let parsed: PluginToHost = from_str(&json).unwrap();
    assert_eq!(msg, parsed);
}

#[test]
fn plugin_list_conversations_with_id() {
    let json = r#"{"type":"list_conversations","id":"req-1"}"#;
    let msg: PluginToHost = from_str(json).unwrap();
    if let PluginToHost::ListConversations(req) = msg {
        assert_eq!(req.id.as_deref(), Some("req-1"));
    } else {
        panic!("expected ListConversations");
    }
}

#[test]
fn plugin_read_events_roundtrip() {
    let msg = PluginToHost::ReadEvents(ReadEventsRequest {
        id: None,
        conversation: "17127583920".to_owned(),
    });
    let json = serde_json::to_string(&msg).unwrap();
    let parsed: PluginToHost = from_str(&json).unwrap();
    assert_eq!(msg, parsed);
}

#[test]
fn plugin_print_defaults() {
    let json = r#"{"type":"print","text":"hello\n"}"#;
    let msg: PluginToHost = from_str(json).unwrap();
    if let PluginToHost::Print(print) = msg {
        assert_eq!(print.text, "hello\n");
        assert_eq!(print.channel, "content");
        assert_eq!(print.format, "plain");
        assert!(print.language.is_none());
    } else {
        panic!("expected Print");
    }
}

#[test]
fn plugin_exit_roundtrip() {
    let msg = PluginToHost::Exit(ExitMessage {
        code: 0,
        reason: None,
    });
    let json = serde_json::to_string(&msg).unwrap();
    // reason: None should not appear in JSON
    assert!(!json.contains("reason"));
    let parsed: PluginToHost = from_str(&json).unwrap();
    assert_eq!(msg, parsed);
}

#[test]
fn plugin_exit_with_reason() {
    let msg = PluginToHost::Exit(ExitMessage {
        code: 1,
        reason: Some("something went wrong".to_owned()),
    });
    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains("reason"));
    let parsed: PluginToHost = from_str(&json).unwrap();
    assert_eq!(msg, parsed);
}

#[test]
fn plugin_exit_without_reason_deserializes() {
    // Old-format exit message without reason field.
    let json = r#"{"type":"exit","code":0}"#;
    let msg: PluginToHost = from_str(json).unwrap();
    if let PluginToHost::Exit(exit) = msg {
        assert_eq!(exit.code, 0);
        assert!(exit.reason.is_none());
    } else {
        panic!("expected Exit");
    }
}

#[test]
fn host_error_roundtrip() {
    let msg = HostToPlugin::Error(ErrorResponse {
        id: Some("a".to_owned()),
        request: Some("read_events".to_owned()),
        message: "not found".to_owned(),
    });
    let json = serde_json::to_string(&msg).unwrap();
    let parsed: HostToPlugin = from_str(&json).unwrap();
    assert_eq!(msg, parsed);
}

#[test]
fn host_describe_roundtrip() {
    let msg = HostToPlugin::Describe;
    let json = serde_json::to_string(&msg).unwrap();
    assert_eq!(json, r#"{"type":"describe"}"#);
    let parsed: HostToPlugin = from_str(&json).unwrap();
    assert_eq!(msg, parsed);
}

#[test]
fn plugin_describe_roundtrip() {
    let msg = PluginToHost::Describe(DescribeResponse {
        name: "serve".to_owned(),
        version: "0.1.0".to_owned(),
        description: "Web UI for conversations".to_owned(),
        command: vec![],
        author: Some("Test Author".to_owned()),
        help: Some("Full help text here".to_owned()),
        repository: None,
    });
    let json = serde_json::to_string(&msg).unwrap();
    let parsed: PluginToHost = from_str(&json).unwrap();
    assert_eq!(msg, parsed);
    // Empty vecs and None fields should not appear in JSON.
    assert!(!json.contains("repository"));
    assert!(!json.contains("command"));
}

#[test]
fn plugin_describe_with_command_path() {
    let msg = PluginToHost::Describe(DescribeResponse {
        name: "serve-web".to_owned(),
        version: "0.1.0".to_owned(),
        description: "Web UI server".to_owned(),
        command: vec!["serve".to_owned(), "web".to_owned()],
        author: None,
        help: None,
        repository: None,
    });
    let json = serde_json::to_string(&msg).unwrap();
    let parsed: PluginToHost = from_str(&json).unwrap();
    assert_eq!(msg, parsed);
    assert!(json.contains(r#"command":["serve","web"]"#));
}

#[test]
fn plugin_describe_without_command_deserializes() {
    // Simple plugins can omit the `command` field entirely.
    let json = r#"{"type":"describe","name":"serve","version":"0.1.0","description":"test"}"#;
    let msg: PluginToHost = from_str(json).unwrap();
    if let PluginToHost::Describe(desc) = msg {
        assert!(desc.command.is_empty());
    } else {
        panic!("expected Describe");
    }
}

#[test]
fn host_shutdown_roundtrip() {
    let msg = HostToPlugin::Shutdown;
    let json = serde_json::to_string(&msg).unwrap();
    assert_eq!(json, r#"{"type":"shutdown"}"#);
    let parsed: HostToPlugin = from_str(&json).unwrap();
    assert_eq!(msg, parsed);
}

#[test]
fn plugin_log_roundtrip() {
    let msg = PluginToHost::Log(LogMessage {
        level: "info".to_owned(),
        message: "started".to_owned(),
        fields: Map::new(),
    });
    let json = serde_json::to_string(&msg).unwrap();
    let parsed: PluginToHost = from_str(&json).unwrap();
    assert_eq!(msg, parsed);
}

#[test]
fn plugin_read_config_with_path() {
    let msg = PluginToHost::ReadConfig(ReadConfigRequest {
        id: None,
        path: Some("assistant.model".to_owned()),
    });
    let json = serde_json::to_string(&msg).unwrap();
    let parsed: PluginToHost = from_str(&json).unwrap();
    assert_eq!(msg, parsed);
}
