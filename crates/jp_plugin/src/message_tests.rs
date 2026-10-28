use pretty_assertions::assert_eq;
use serde_json::{Map, from_str, json};

use super::*;

#[test]
fn host_init_roundtrip() {
    let msg = HostToPlugin::Init(Box::new(InitMessage {
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
        output_format: OutputFormat::JsonPretty,
        tool: None,
    }));

    let json = serde_json::to_string(&msg).unwrap();
    let parsed: HostToPlugin = from_str(&json).unwrap();
    assert_eq!(msg, parsed);
}

/// The wire shape a plugin serving tools parses, pinned so a field renamed on
/// the host side shows up here rather than as a plugin that silently stops
/// answering.
#[test]
fn host_init_with_a_tool_call_serializes_to_the_documented_shape() {
    let msg = HostToPlugin::Init(Box::new(InitMessage {
        version: 12,
        workspace: WorkspaceInfo {
            root: "/project".into(),
            storage: "/project/.jp".into(),
            id: "abc12".to_owned(),
        },
        paths: PathsInfo::default(),
        config: json!({}),
        options: Map::from_iter([("dir".to_owned(), json!("packages/foo/tickets"))]),
        args: vec![],
        log_level: 0,
        output_format: OutputFormat::Text,
        tool: Some(ToolCall {
            action: ToolAction::FormatArguments,
            name: "create".to_owned(),
            arguments: Map::from_iter([("title".to_owned(), json!("Fix it"))]),
            answers: Map::new(),
            options: Map::new(),
            access: Some(json!({"fs": [], "net": [], "env": [{"name": "AWS_*", "read": false}]})),
            conversation: "jp-c17000000000".to_owned(),
        }),
    }));

    assert_eq!(
        serde_json::to_value(&msg).unwrap(),
        json!({
            "type": "init",
            "version": 12,
            "workspace": {"root": "/project", "storage": "/project/.jp", "id": "abc12"},
            "paths": {},
            "config": {},
            "options": {"dir": "packages/foo/tickets"},
            "args": [],
            "log_level": 0,
            "output_format": "text",
            "tool": {
                "action": "format_arguments",
                "name": "create",
                "arguments": {"title": "Fix it"},
                "access": {"fs": [], "net": [], "env": [{"name": "AWS_*", "read": false}]},
                "conversation": "jp-c17000000000"
            }
        })
    );
}

/// A tool call that names no action is a run.
#[test]
fn tool_call_without_an_action_is_a_run() {
    let call: ToolCall = from_str(r#"{"name":"create"}"#).unwrap();

    assert_eq!(call.action, ToolAction::Run);
    assert_eq!(
        serde_json::to_value(&call).unwrap(),
        json!({"action": "run", "name": "create", "arguments": {}})
    );
}

#[test]
fn host_init_without_a_tool_call_reads_as_a_command() {
    let json = r#"{"type":"init","version":9,"workspace":{"root":"/p","storage":"/p/.jp","id":"x"},"config":{},"args":["list"]}"#;
    let HostToPlugin::Init(init) = from_str(json).unwrap() else {
        panic!("expected Init");
    };

    assert_eq!(init.tool, None);
}

#[test]
fn plugin_tool_outcome_roundtrip() {
    let msg = PluginToHost::ToolOutcome(ToolOutcomeMessage {
        outcome: json!({"type": "success", "content": "Created T-0abc123"}),
    });
    // Compared as values: key order inside `outcome` depends on whether
    // `serde_json`'s `preserve_order` is unified into this build.
    let json = serde_json::to_string(&msg).unwrap();
    assert_eq!(
        from_str::<Value>(&json).unwrap(),
        json!({"type": "tool_outcome", "outcome": {"type": "success", "content": "Created T-0abc123"}})
    );

    let parsed: PluginToHost = from_str(&json).unwrap();
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
    let msg = PluginToHost::Ready(ReadyMessage { protocol: 2 });
    let json = serde_json::to_string(&msg).unwrap();
    assert_eq!(json, r#"{"type":"ready","protocol":2}"#);

    let parsed: PluginToHost = from_str(&json).unwrap();
    assert_eq!(msg, parsed);
}

/// A plugin built before the field existed sends a bare `ready`, and is assumed
/// to need protocol 1.
///
/// An assumption, not a report: it needs whatever it was built against, which
/// may be more than 1, and it has no way to say so.
#[test]
fn plugin_ready_without_a_protocol_reads_as_the_first_version() {
    let parsed: PluginToHost = from_str(r#"{"type":"ready"}"#).unwrap();

    assert_eq!(parsed, PluginToHost::Ready(ReadyMessage { protocol: 1 }));
}

/// A host built before the field existed still reads a `ready` that carries
/// one.
///
/// The other direction of the same drift: the plugin is updated and `jp` is
/// not.
/// `LegacyPluginToHost` is the shape such a host compiled against, so this
/// holds it to the compatibility this design assumes rather than to whatever
/// serde happens to do with an unknown key.
#[test]
fn a_ready_carrying_a_protocol_still_reads_on_a_host_that_predates_it() {
    #[derive(Debug, serde::Deserialize, PartialEq)]
    #[serde(tag = "type", rename_all = "snake_case")]
    enum LegacyPluginToHost {
        Ready,
        Exit(ExitMessage),
    }

    let parsed: LegacyPluginToHost = from_str(r#"{"type":"ready","protocol":2}"#).unwrap();

    assert_eq!(
        parsed,
        LegacyPluginToHost::Ready,
        "an older host reads the version field as an unknown key, not as a parse failure"
    );
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

/// The manifest's fields sit beside the rest on the wire, not nested under a
/// key of their own.
#[test]
fn plugin_describe_carries_the_manifest_fields_flat() {
    let msg = PluginToHost::Describe(DescribeResponse {
        manifest: crate::Manifest {
            protocol: 9,
            description: "Web UI server".to_owned(),
            command: vec!["serve".to_owned(), "web".to_owned()],
        },
        name: "serve-web".to_owned(),
        version: "0.1.0".to_owned(),
        help: "Usage: jp serve web".to_owned(),
        author: None,
        repository: None,
    });

    let json = serde_json::to_value(&msg).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "type": "describe",
            "protocol": 9,
            "description": "Web UI server",
            "command": ["serve", "web"],
            "name": "serve-web",
            "version": "0.1.0",
            "help": "Usage: jp serve web",
        })
    );

    let parsed: PluginToHost = serde_json::from_value(json).unwrap();
    assert_eq!(msg, parsed);
}

#[test]
fn plugin_describe_without_help_is_refused() {
    let json = r#"{"type":"describe","protocol":1,"name":"serve","version":"0.1.0","description":"test","command":["serve"]}"#;

    assert!(from_str::<PluginToHost>(json).is_err());
}

#[test]
fn describe_is_built_from_the_manifest_line() {
    const MANIFEST: &str = crate::manifest!(
        protocol: 2,
        description: "Track work items",
        command: ["ticket"],
    );

    let describe =
        DescribeResponse::from_manifest(MANIFEST, "ticket", "0.1.0", "Usage: jp ticket").unwrap();

    assert_eq!(describe.manifest.protocol, 2);
    assert_eq!(describe.manifest.description, "Track work items");
    assert_eq!(describe.manifest.command, ["ticket"]);
    assert_eq!(describe.help, "Usage: jp ticket");
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
        reload: false,
    });
    let json = serde_json::to_string(&msg).unwrap();
    let parsed: PluginToHost = from_str(&json).unwrap();
    assert_eq!(msg, parsed);
}

#[test]
fn plugin_read_config_reload_is_omitted_unless_set() {
    let plain = PluginToHost::ReadConfig(ReadConfigRequest {
        id: None,
        path: None,
        reload: false,
    });
    assert_eq!(
        serde_json::to_string(&plain).unwrap(),
        r#"{"type":"read_config"}"#
    );

    let reload = PluginToHost::ReadConfig(ReadConfigRequest {
        id: None,
        path: None,
        reload: true,
    });
    assert_eq!(
        serde_json::to_string(&reload).unwrap(),
        r#"{"type":"read_config","reload":true}"#
    );
    assert_eq!(
        from_str::<PluginToHost>(r#"{"type":"read_config","reload":true}"#).unwrap(),
        reload
    );
}

/// A summary with fixed timestamps, so the expected JSON below is a literal.
fn pinned_summary(pinned_at: Option<&str>) -> ConversationSummary {
    ConversationSummary {
        id: "123".to_owned(),
        title: None,
        last_activated_at: "2024-09-02T12:30:00Z".parse().unwrap(),
        pinned_at: pinned_at.map(|at| at.parse().unwrap()),
        events_count: 0,
    }
}

#[test]
fn conversation_summary_emits_pinned_at_when_set() {
    assert_eq!(
        serde_json::to_string(&pinned_summary(Some("2024-09-03T08:00:00Z"))).unwrap(),
        r#"{"id":"123","title":null,"last_activated_at":"2024-09-02T12:30:00Z","pinned_at":"2024-09-03T08:00:00Z","events_count":0}"#
    );
}

#[test]
fn conversation_summary_omits_pinned_at_when_unset() {
    assert_eq!(
        serde_json::to_string(&pinned_summary(None)).unwrap(),
        r#"{"id":"123","title":null,"last_activated_at":"2024-09-02T12:30:00Z","events_count":0}"#
    );
}

/// A host built before `pinned_at` existed sends no such key, and a plugin
/// built after it has to keep reading those messages.
#[test]
fn conversation_summary_defaults_pinned_at_when_absent() {
    let json =
        r#"{"id":"123","title":null,"last_activated_at":"2024-09-02T12:30:00Z","events_count":0}"#;

    assert_eq!(
        from_str::<ConversationSummary>(json).unwrap(),
        pinned_summary(None)
    );
}

#[test]
fn a_structured_temporary_query_reads_from_json() {
    let json = r#"{"type":"query","id":"q1","content":"Summarize","new":true,"schema":{"type":"object"},"expires_in":"5m"}"#;

    assert_eq!(
        from_str::<PluginToHost>(json).unwrap(),
        PluginToHost::Query(QueryRequest {
            id: Some("q1".to_owned()),
            conversation: String::new(),
            content: "Summarize".to_owned(),
            new: true,
            title: None,
            cfg: vec![],
            schema: Some(Map::from_iter([("type".to_owned(), json!("object"))])),
            expires_in: Some("5m".to_owned()),
        })
    );
}

/// A plugin that asks for neither sends the same bytes it sent before the
/// fields existed, so an older host still reads it.
#[test]
fn a_plain_query_writes_neither_schema_nor_expires_in() {
    let msg = PluginToHost::Query(QueryRequest {
        id: None,
        conversation: "123".to_owned(),
        content: "hi".to_owned(),
        new: false,
        title: None,
        cfg: vec![],
        schema: None,
        expires_in: None,
    });

    assert_eq!(
        serde_json::to_string(&msg).unwrap(),
        r#"{"type":"query","conversation":"123","content":"hi"}"#
    );
}

/// A schema that admits `null`, such as `{"type": ["string", "null"]}`, makes
/// `null` a valid answer, and it has to stay distinct from a reply carrying no
/// answer at all.
#[test]
fn a_null_answer_survives_the_round_trip() {
    let msg = HostToPlugin::QueryComplete(QueryCompleteResponse {
        id: None,
        conversation: "123".to_owned(),
        data: Some(Value::Null),
    });

    let json = serde_json::to_string(&msg).unwrap();
    assert_eq!(
        json,
        r#"{"type":"query_complete","conversation":"123","data":null}"#
    );
    assert_eq!(from_str::<HostToPlugin>(&json).unwrap(), msg);

    assert_eq!(
        from_str::<HostToPlugin>(r#"{"type":"query_complete","conversation":"123"}"#).unwrap(),
        HostToPlugin::QueryComplete(QueryCompleteResponse {
            id: None,
            conversation: "123".to_owned(),
            data: None,
        })
    );
}

#[test]
fn query_complete_carries_data_only_when_there_is_some() {
    let without = HostToPlugin::QueryComplete(QueryCompleteResponse {
        id: None,
        conversation: "123".to_owned(),
        data: None,
    });
    let with = HostToPlugin::QueryComplete(QueryCompleteResponse {
        id: None,
        conversation: "123".to_owned(),
        data: Some(json!({"summary": "short"})),
    });

    assert_eq!(
        serde_json::to_string(&without).unwrap(),
        r#"{"type":"query_complete","conversation":"123"}"#
    );
    assert_eq!(
        serde_json::to_string(&with).unwrap(),
        r#"{"type":"query_complete","conversation":"123","data":{"summary":"short"}}"#
    );
}
