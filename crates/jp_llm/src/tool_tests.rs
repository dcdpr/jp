use async_trait::async_trait;
use jp_config::{
    AppConfig, Config as _,
    conversation::tool::{PartialToolConfig, ToolConfig},
};
use jp_mcp::Client;
use jp_tool::Outcome;

use super::*;

struct EchoArguments;

#[async_trait]
impl BuiltinTool for EchoArguments {
    async fn execute(&self, arguments: &Value, _answers: &IndexMap<String, Value>) -> Outcome {
        Outcome::Success {
            content: arguments.to_string(),
        }
    }
}

#[test]
fn test_execution_outcome_completed_success_into_response() {
    let outcome = ExecutionOutcome::Completed {
        id: "call_123".to_string(),
        result: Ok("Tool output".to_string()),
    };

    let response = outcome.into_response();
    assert_eq!(response.id, "call_123");
    assert_eq!(response.result, Ok("Tool output".to_string()));
}

#[test]
fn test_execution_outcome_completed_error_into_response() {
    let outcome = ExecutionOutcome::Completed {
        id: "call_456".to_string(),
        result: Err("Tool failed".to_string()),
    };

    let response = outcome.into_response();
    assert_eq!(response.id, "call_456");
    assert_eq!(response.result, Err("Tool failed".to_string()));
}

#[test]
fn test_execution_outcome_needs_input_into_response() {
    let question = Question::text("q1", "What is your name?");

    let outcome = ExecutionOutcome::NeedsInput {
        id: "call_789".to_string(),
        question,
    };

    let response = outcome.into_response();
    assert_eq!(response.id, "call_789");
    assert!(response.result.is_ok());
    assert!(
        response
            .result
            .unwrap()
            .contains("requires additional input")
    );
}

#[test]
fn test_execution_outcome_cancelled_into_response() {
    let outcome = ExecutionOutcome::Cancelled {
        id: "call_abc".to_string(),
    };

    let response = outcome.into_response();
    assert_eq!(response.id, "call_abc");
    assert!(response.result.is_ok());
    assert!(response.result.unwrap().contains("cancelled"));
}

#[test]
fn test_execution_outcome_id() {
    let completed = ExecutionOutcome::Completed {
        id: "id1".to_string(),
        result: Ok(String::new()),
    };
    assert_eq!(completed.id(), "id1");

    let needs_input = ExecutionOutcome::NeedsInput {
        id: "id2".to_string(),
        question: Question::text("q", "?"),
    };
    assert_eq!(needs_input.id(), "id2");

    let cancelled = ExecutionOutcome::Cancelled {
        id: "id3".to_string(),
    };
    assert_eq!(cancelled.id(), "id3");
}

#[test]
fn test_execution_outcome_helper_methods() {
    let success = ExecutionOutcome::Completed {
        id: "1".to_string(),
        result: Ok("output".to_string()),
    };
    assert!(success.is_success());
    assert!(!success.needs_input());
    assert!(!success.is_cancelled());

    let failure = ExecutionOutcome::Completed {
        id: "2".to_string(),
        result: Err("error".to_string()),
    };
    assert!(!failure.is_success());
    assert!(!failure.needs_input());
    assert!(!failure.is_cancelled());

    let needs_input = ExecutionOutcome::NeedsInput {
        id: "3".to_string(),
        question: Question::boolean("q", "?"),
    };
    assert!(!needs_input.is_success());
    assert!(needs_input.needs_input());
    assert!(!needs_input.is_cancelled());

    let cancelled = ExecutionOutcome::Cancelled {
        id: "4".to_string(),
    };
    assert!(!cancelled.is_success());
    assert!(!cancelled.needs_input());
    assert!(cancelled.is_cancelled());
}

/// Build a minimal `ToolParameterSchema` for use in validation tests.
fn param(kind: &str, required: bool) -> ToolParameterSchema {
    ToolParameterSchema {
        kind: kind.to_owned().into(),
        required,
        default: None,
        summary: None,
        description: None,
        examples: None,
        enumeration: vec![],
        items: None,
        properties: IndexMap::default(),
    }
}

#[tokio::test]
async fn local_tool_rejects_scalar_enum_on_array_parameter() {
    let partial: PartialToolConfig = serde_json::from_value(json!({
        "source": "local",
        "parameters": {
            "tags": {
                "type": "array",
                "enum": ["projects/jp", "task", "idea"],
                "items": { "type": "string" }
            }
        }
    }))
    .unwrap();
    let tool = ToolConfig::from_partial(partial, vec![]).unwrap();
    let mut app = AppConfig::new_test();
    app.conversation
        .tools
        .insert("bear_note_create".to_owned(), tool);
    let config = app.conversation.tools.get("bear_note_create").unwrap();

    let error = resolve_tool("bear_note_create", &config, &Client::new(IndexMap::new()))
        .await
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "Invalid schema at `conversation.tools.bear_note_create.parameters.tags.enum`: enum value \
         \"projects/jp\" has type string, but the schema requires array; use \
         `conversation.tools.bear_note_create.parameters.tags.items.enum` to constrain array \
         elements"
    );
}

#[test]
fn coerces_json_strings_to_declared_parameter_types() {
    let parameters = IndexMap::from_iter([
        ("path".to_owned(), param("string", true)),
        ("start_line".to_owned(), param("integer", false)),
        ("enabled".to_owned(), param("boolean", false)),
        ("string_or_integer".to_owned(), ToolParameterSchema {
            kind: vec!["string".to_owned(), "integer".to_owned()].into(),
            ..param("string", false)
        }),
        ("patterns".to_owned(), ToolParameterSchema {
            kind: "array".to_owned().into(),
            items: Some(Box::new(ToolParameterSchema {
                kind: "object".to_owned().into(),
                properties: IndexMap::from_iter([("count".to_owned(), param("integer", true))]),
                ..param("object", false)
            })),
            ..param("array", false)
        }),
    ]);
    let mut arguments = json!({
        "path": "README.md",
        "start_line": "1",
        "enabled": "true",
        "string_or_integer": "3",
        "patterns": "[{\"count\":\"2\"}]"
    })
    .as_object()
    .cloned()
    .unwrap();

    ToolDefinition {
        name: "test".to_owned(),
        docs: ToolDocs::default(),
        parameters,
    }
    .coerce_arguments(&mut arguments);

    assert_eq!(
        Value::Object(arguments),
        json!({
            "path": "README.md",
            "start_line": 1,
            "enabled": true,
            "string_or_integer": "3",
            "patterns": [{"count": 2}]
        })
    );
}

#[tokio::test]
async fn execute_coerces_json_strings_before_calling_tool() {
    let partial: PartialToolConfig = serde_json::from_value(json!({
        "source": "builtin",
    }))
    .unwrap();
    let tool = ToolConfig::from_partial(partial, vec![]).unwrap();
    let mut app = AppConfig::new_test();
    app.conversation
        .tools
        .insert("echo_arguments".to_owned(), tool);
    let config = app.conversation.tools.get("echo_arguments").unwrap();
    let definition = ToolDefinition {
        name: "echo_arguments".to_owned(),
        docs: ToolDocs::default(),
        parameters: IndexMap::from_iter([("start_line".to_owned(), param("integer", false))]),
    };
    let builtins = builtin::BuiltinExecutors::new().register("echo_arguments", EchoArguments);

    let outcome = definition
        .execute(
            "call_1".to_owned(),
            json!({"start_line": "1"}),
            &IndexMap::new(),
            &config,
            &Client::new(IndexMap::new()),
            Utf8Path::new("/tmp"),
            CancellationToken::new(),
            &builtins,
            None,
            &InvocationContext::default(),
        )
        .await
        .unwrap();

    let ExecutionOutcome::Completed { id, result } = outcome else {
        panic!("expected completed tool call");
    };
    assert_eq!(id, "call_1");
    assert_eq!(result, Ok(r#"{"start_line":1}"#.to_owned()));
}

#[test]
fn test_validate_tool_arguments() {
    struct TestCase {
        arguments: Map<String, Value>,
        parameters: IndexMap<String, ToolParameterSchema>,
        want: Result<(), ToolError>,
    }

    let cases = vec![
        ("empty", TestCase {
            arguments: Map::new(),
            parameters: IndexMap::new(),
            want: Ok(()),
        }),
        ("correct", TestCase {
            arguments: Map::from_iter([("foo".to_owned(), json!("bar"))]),
            parameters: IndexMap::from_iter([
                ("foo".to_owned(), param("string", true)),
                ("bar".to_owned(), param("string", false)),
            ]),
            want: Ok(()),
        }),
        ("missing", TestCase {
            arguments: Map::new(),
            parameters: IndexMap::from_iter([("foo".to_owned(), param("string", true))]),
            want: Err(ToolError::Arguments {
                missing: vec!["foo".to_owned()],
                unknown: vec![],
            }),
        }),
        ("unknown", TestCase {
            arguments: Map::from_iter([("foo".to_owned(), json!("bar"))]),
            parameters: IndexMap::from_iter([("bar".to_owned(), param("string", false))]),
            want: Err(ToolError::Arguments {
                missing: vec![],
                unknown: vec!["foo".to_owned()],
            }),
        }),
        ("both", TestCase {
            arguments: Map::from_iter([("foo".to_owned(), json!("bar"))]),
            parameters: IndexMap::from_iter([("bar".to_owned(), param("string", true))]),
            want: Err(ToolError::Arguments {
                missing: vec!["bar".to_owned()],
                unknown: vec!["foo".to_owned()],
            }),
        }),
    ];

    for (name, test_case) in cases {
        let result = validate_tool_arguments(&test_case.arguments, &test_case.parameters);
        assert_eq!(result, test_case.want, "failed case: {name}");
    }
}

#[test]
fn test_validate_nested_array_item_properties() {
    // Mirrors the fs_modify_file schema:
    //   patterns: array of { old: string (required), new: string (required) }
    let parameters = IndexMap::from_iter([
        ("path".to_owned(), param("string", true)),
        ("patterns".to_owned(), ToolParameterSchema {
            kind: "array".to_owned().into(),
            required: true,
            items: Some(Box::new(ToolParameterSchema {
                kind: "object".to_owned().into(),
                required: false,
                properties: IndexMap::from_iter([
                    ("old".to_owned(), param("string", true)),
                    ("new".to_owned(), param("string", true)),
                ]),
                ..param("object", false)
            })),
            ..param("array", true)
        }),
    ]);

    // Valid: correct inner fields.
    let args = json!({
        "path": "src/lib.rs",
        "patterns": [{"old": "foo", "new": "bar"}]
    });
    assert_eq!(
        validate_tool_arguments(args.as_object().unwrap(), &parameters),
        Ok(())
    );

    // Valid: multiple items.
    let args = json!({
        "path": "src/lib.rs",
        "patterns": [
            {"old": "a", "new": "b"},
            {"old": "c", "new": "d"}
        ]
    });
    assert_eq!(
        validate_tool_arguments(args.as_object().unwrap(), &parameters),
        Ok(())
    );

    // Invalid: unknown inner field.
    let args = json!({
        "path": "src/lib.rs",
        "patterns": [{"old": "foo", "new": "bar", "extra": true}]
    });
    assert_eq!(
        validate_tool_arguments(args.as_object().unwrap(), &parameters),
        Err(ToolError::Arguments {
            missing: vec![],
            unknown: vec!["extra".to_owned()],
        })
    );

    // Invalid: missing required inner field.
    let args = json!({
        "path": "src/lib.rs",
        "patterns": [{"old": "foo"}]
    });
    assert_eq!(
        validate_tool_arguments(args.as_object().unwrap(), &parameters),
        Err(ToolError::Arguments {
            missing: vec!["new".to_owned()],
            unknown: vec![],
        })
    );

    // Invalid: wrong inner field names (the LLM hallucinated names).
    let args = json!({
        "path": "src/lib.rs",
        "patterns": [{"string_to_replace": "foo", "new_string": "bar"}]
    });
    let err = validate_tool_arguments(args.as_object().unwrap(), &parameters);
    assert!(err.is_err());
    let ToolError::Arguments { missing, unknown } = err.unwrap_err() else {
        panic!("expected Arguments error");
    };
    assert_eq!(missing, vec!["old".to_owned(), "new".to_owned()]);
    // preserve_order: keys iterate in insertion order from json! macro
    assert_eq!(unknown, vec![
        "string_to_replace".to_owned(),
        "new_string".to_owned()
    ]);

    // Valid: non-object array items are skipped (no crash).
    let args = json!({
        "path": "src/lib.rs",
        "patterns": ["not an object"]
    });
    assert_eq!(
        validate_tool_arguments(args.as_object().unwrap(), &parameters),
        Ok(())
    );

    // Valid: parameter is not an array (type mismatch, but not our job to check types).
    let args = json!({
        "path": "src/lib.rs",
        "patterns": "not an array"
    });
    assert_eq!(
        validate_tool_arguments(args.as_object().unwrap(), &parameters),
        Ok(())
    );
}

#[test]
fn test_validate_nested_object_properties() {
    let parameters = IndexMap::from_iter([
        ("name".to_owned(), param("string", true)),
        ("config".to_owned(), ToolParameterSchema {
            kind: "object".to_owned().into(),
            required: false,
            properties: IndexMap::from_iter([
                ("verbose".to_owned(), param("boolean", false)),
                ("output".to_owned(), param("string", true)),
            ]),
            ..param("object", false)
        }),
    ]);

    // Valid.
    let args = json!({ "name": "test", "config": { "verbose": true, "output": "out.txt" } });
    assert_eq!(
        validate_tool_arguments(args.as_object().unwrap(), &parameters),
        Ok(())
    );

    // Valid: optional object param omitted entirely.
    let args = json!({ "name": "test" });
    assert_eq!(
        validate_tool_arguments(args.as_object().unwrap(), &parameters),
        Ok(())
    );

    // Invalid: unknown field inside the object.
    let args = json!({ "name": "test", "config": { "output": "o", "bogus": 1 } });
    assert_eq!(
        validate_tool_arguments(args.as_object().unwrap(), &parameters),
        Err(ToolError::Arguments {
            missing: vec![],
            unknown: vec!["bogus".to_owned()],
        })
    );

    // Invalid: missing required field inside the object.
    let args = json!({ "name": "test", "config": { "verbose": true } });
    assert_eq!(
        validate_tool_arguments(args.as_object().unwrap(), &parameters),
        Err(ToolError::Arguments {
            missing: vec!["output".to_owned()],
            unknown: vec![],
        })
    );
}

/// Build a parameter with a default value.
fn param_with_default(kind: &str, required: bool, default: Value) -> ToolParameterSchema {
    ToolParameterSchema {
        default: Some(default),
        ..param(kind, required)
    }
}

#[test]
fn test_apply_defaults_fills_missing_required_with_default() {
    let parameters = IndexMap::from_iter([
        ("path".to_owned(), param("string", true)),
        (
            "use_regex".to_owned(),
            param_with_default("boolean", true, json!(false)),
        ),
    ]);

    let mut args: Map<String, Value> = Map::from_iter([("path".to_owned(), json!("src/lib.rs"))]);

    apply_parameter_defaults(&mut args, &parameters);

    assert_eq!(args.get("path"), Some(&json!("src/lib.rs")));
    assert_eq!(args.get("use_regex"), Some(&json!(false)));
}

#[test]
fn test_apply_defaults_does_not_overwrite_provided_values() {
    let parameters = IndexMap::from_iter([(
        "use_regex".to_owned(),
        param_with_default("boolean", true, json!(false)),
    )]);

    let mut args: Map<String, Value> = Map::from_iter([("use_regex".to_owned(), json!(true))]);

    apply_parameter_defaults(&mut args, &parameters);

    assert_eq!(args.get("use_regex"), Some(&json!(true)));
}

#[test]
fn test_apply_defaults_fills_optional_param_with_default() {
    let parameters = IndexMap::from_iter([(
        "verbose".to_owned(),
        param_with_default("boolean", false, json!(false)),
    )]);

    let mut args: Map<String, Value> = Map::new();
    apply_parameter_defaults(&mut args, &parameters);

    assert_eq!(args.get("verbose"), Some(&json!(false)));
}

#[test]
fn test_apply_defaults_skips_params_without_default() {
    let parameters = IndexMap::from_iter([("path".to_owned(), param("string", true))]);

    let mut args: Map<String, Value> = Map::new();
    apply_parameter_defaults(&mut args, &parameters);

    assert!(!args.contains_key("path"));
}

#[test]
fn test_apply_defaults_recurses_into_objects() {
    let parameters = IndexMap::from_iter([("config".to_owned(), ToolParameterSchema {
        kind: "object".to_owned().into(),
        required: false,
        properties: IndexMap::from_iter([(
            "verbose".to_owned(),
            param_with_default("boolean", false, json!(true)),
        )]),
        ..param("object", false)
    })]);

    let mut args: Map<String, Value> = Map::from_iter([("config".to_owned(), json!({}))]);

    apply_parameter_defaults(&mut args, &parameters);

    assert_eq!(args["config"]["verbose"], json!(true));
}

#[test]
fn test_apply_defaults_recurses_into_array_items() {
    let parameters = IndexMap::from_iter([("items".to_owned(), ToolParameterSchema {
        kind: "array".to_owned().into(),
        required: true,
        items: Some(Box::new(ToolParameterSchema {
            kind: "object".to_owned().into(),
            required: false,
            properties: IndexMap::from_iter([(
                "enabled".to_owned(),
                param_with_default("boolean", false, json!(true)),
            )]),
            ..param("object", false)
        })),
        ..param("array", true)
    })]);

    let mut args: Map<String, Value> = Map::from_iter([(
        "items".to_owned(),
        json!([{"name": "a"}, {"name": "b", "enabled": false}]),
    )]);

    apply_parameter_defaults(&mut args, &parameters);

    let items = args["items"].as_array().unwrap();
    assert_eq!(items[0]["enabled"], json!(true));
    // Explicitly provided false is preserved.
    assert_eq!(items[1]["enabled"], json!(false));
}

#[test]
fn test_apply_defaults_then_validate_passes() {
    // Mirrors the fs_modify_file scenario: replace_using_regex is required
    // with a default, and the LLM omits it.
    let parameters = IndexMap::from_iter([
        ("path".to_owned(), param("string", true)),
        (
            "replace_using_regex".to_owned(),
            param_with_default("boolean", true, json!(false)),
        ),
    ]);

    let mut args: Map<String, Value> = Map::from_iter([("path".to_owned(), json!("README.md"))]);

    // Without defaults, validation would fail.
    assert!(validate_tool_arguments(&args, &parameters).is_err());

    // After applying defaults, validation passes.
    apply_parameter_defaults(&mut args, &parameters);
    assert!(validate_tool_arguments(&args, &parameters).is_ok());
    assert_eq!(args["replace_using_regex"], json!(false));
}

#[test]
fn test_split_short_single_line() {
    let (s, d) = split_description("Run cargo check.");
    assert_eq!(s, "Run cargo check.");
    assert_eq!(d, None);
}

#[test]
fn test_split_short_no_period() {
    let (s, d) = split_description("Run cargo check");
    assert_eq!(s, "Run cargo check");
    assert_eq!(d, None);
}

#[test]
fn test_split_two_sentences() {
    let (s, d) = split_description(
        "Run cargo check on a package. Supports workspace packages and feature flags.",
    );
    assert_eq!(s, "Run cargo check on a package.");
    assert_eq!(
        d,
        Some("Supports workspace packages and feature flags.".to_owned())
    );
}

#[test]
fn test_split_multiline() {
    let input = "Search for code in a repository.\n\nSupports regex and qualifiers.";
    let (s, d) = split_description(input);
    assert_eq!(s, "Search for code in a repository.");
    assert_eq!(d, Some("Supports regex and qualifiers.".to_owned()));
}

#[test]
fn test_split_multiline_no_period() {
    let input = "First line without period\nSecond line here.";
    let (s, d) = split_description(input);
    assert_eq!(s, "First line without period");
    assert_eq!(d, Some("Second line here.".to_owned()));
}

#[test]
fn test_split_preserves_abbreviations() {
    // "e.g." should not be treated as a sentence boundary.
    let (s, d) = split_description("Use e.g. foo or bar.");
    assert_eq!(s, "Use e.g. foo or bar.");
    assert_eq!(d, None);
}

#[test]
fn test_split_long_single_line_with_period() {
    let input = "This is a very long description that exceeds the threshold. It contains \
                 additional details about the tool's behavior.";
    let (s, d) = split_description(input);
    assert_eq!(
        s,
        "This is a very long description that exceeds the threshold."
    );
    assert!(d.is_some());
}

#[test]
fn test_split_empty() {
    let (s, d) = split_description("");
    assert_eq!(s, "");
    assert_eq!(d, None);
}

#[test]
fn test_split_trims_whitespace() {
    let (s, d) = split_description("  hello  ");
    assert_eq!(s, "hello");
    assert_eq!(d, None);
}

/// Regression: `{{tool}}` must render as valid JSON, including `null` for null
/// fields (not Jinja2's `none`).
/// Originally fixed with `AutoEscape::Json`, now handled by the custom
/// formatter which JSON-serializes composite values while leaving scalars
/// alone.
#[tokio::test]
#[cfg(unix)]
async fn test_run_tool_command_renders_null_args_as_valid_json() {
    use jp_config::conversation::tool::CommandConfig;

    let ctx = json!({
        "tool": {
            "name": "cargo_test",
            "arguments": {
                "package": "jp_workspace",
                "backtrace": null,
                "testname": null,
            },
            "answers": {},
            "options": {},
        },
        "context": {
            "action": "run",
            "root": "/tmp",
        },
    });

    let command = CommandConfig {
        program: "echo".to_owned(),
        args: vec!["{{tool}}".to_owned()],
        shell: false,
    };

    let result = run_tool_command(command, ctx, "/tmp".into(), CancellationToken::new(), None)
        .await
        .unwrap();

    let stdout = match result {
        CommandResult::RawOutput { stdout, .. } => stdout,
        other => panic!("Expected RawOutput, got: {other:?}"),
    };

    // The rendered output must be valid JSON with proper `null` values.
    let parsed: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!("run_tool_command produced invalid JSON: {e}\n\nOutput: {stdout}")
    });

    assert_eq!(parsed["arguments"]["package"], "jp_workspace");
    assert_eq!(parsed["arguments"]["backtrace"], Value::Null);
    assert_eq!(parsed["name"], "cargo_test");
}

/// Regression: scalar string interpolation must not be JSON-quoted.
/// A prior fix for the null-rendering bug set `AutoEscape::Json` globally,
/// which wrapped every string value in literal `"..."`, breaking templates like
/// `just rfd-draft {{tool.arguments.title}}` where tool authors expect the bare
/// value.
#[tokio::test]
#[cfg(unix)]
async fn test_run_tool_command_renders_scalar_strings_raw() {
    use jp_config::conversation::tool::CommandConfig;

    let ctx = json!({
        "tool": {
            "arguments": { "title": "Hello World" },
        },
    });

    let command = CommandConfig {
        program: "echo".to_owned(),
        args: vec!["{{tool.arguments.title}}".to_owned()],
        shell: false,
    };

    let result = run_tool_command(command, ctx, "/tmp".into(), CancellationToken::new(), None)
        .await
        .unwrap();

    let stdout = match result {
        CommandResult::RawOutput { stdout, .. } => stdout,
        other => panic!("Expected RawOutput, got: {other:?}"),
    };

    assert_eq!(stdout.trim_end(), "Hello World");
}

/// Null scalars render as literal `null` (not Jinja2's `none`, and not an empty
/// string).
/// This keeps the behavior consistent with how null appears inside
/// JSON-serialized composites.
#[tokio::test]
#[cfg(unix)]
async fn test_run_tool_command_renders_null_scalar_as_literal_null() {
    use jp_config::conversation::tool::CommandConfig;

    let ctx = json!({
        "tool": { "arguments": { "maybe": null } },
    });

    let command = CommandConfig {
        program: "echo".to_owned(),
        args: vec!["{{tool.arguments.maybe}}".to_owned()],
        shell: false,
    };

    let result = run_tool_command(command, ctx, "/tmp".into(), CancellationToken::new(), None)
        .await
        .unwrap();

    let stdout = match result {
        CommandResult::RawOutput { stdout, .. } => stdout,
        other => panic!("Expected RawOutput, got: {other:?}"),
    };

    assert_eq!(stdout.trim_end(), "null");
}

/// End-to-end sanity check for the rfd-draft regression: with the old
/// `AutoEscape::Json` behavior, `{{tool.arguments.title}}` rendered as
/// `"Assistant-Initiated ..."` (literal quotes), which then broke the
/// downstream `sed` command inside the just recipe.
/// Verify the title now reaches the subprocess as a clean argument.
#[tokio::test]
#[cfg(unix)]
async fn test_run_tool_command_rfd_draft_title_rendering() {
    use jp_config::conversation::tool::CommandConfig;

    let ctx = json!({
        "tool": {
            "arguments": {
                "variant": "design",
                "title": "Assistant-Initiated User Inquiries via an ask_user Builtin",
            },
        },
    });

    // Mimic the real `just rfd-draft {{variant}} {{title}}` template.
    let command = CommandConfig {
        program: "printf".to_owned(),
        args: vec![
            "%s|%s".to_owned(),
            "{{tool.arguments.variant}}".to_owned(),
            "{{tool.arguments.title}}".to_owned(),
        ],
        shell: false,
    };

    let result = run_tool_command(command, ctx, "/tmp".into(), CancellationToken::new(), None)
        .await
        .unwrap();

    let stdout = match result {
        CommandResult::RawOutput { stdout, .. } => stdout,
        other => panic!("Expected RawOutput, got: {other:?}"),
    };

    assert_eq!(
        stdout,
        "design|Assistant-Initiated User Inquiries via an ask_user Builtin"
    );
}

/// The `tojson` filter still works for tool authors who want explicit
/// JSON-quoted strings (e.g. when hand-crafting a JSON literal).
/// Safe strings produced by `tojson` must pass through the custom formatter
/// unchanged — no double-encoding.
#[tokio::test]
#[cfg(unix)]
async fn test_run_tool_command_tojson_filter_on_scalar_still_works() {
    use jp_config::conversation::tool::CommandConfig;

    let ctx = json!({
        "tool": { "arguments": { "title": "Hello" } },
    });

    let command = CommandConfig {
        program: "echo".to_owned(),
        args: vec!["{{tool.arguments.title | tojson}}".to_owned()],
        shell: false,
    };

    let result = run_tool_command(command, ctx, "/tmp".into(), CancellationToken::new(), None)
        .await
        .unwrap();

    let stdout = match result {
        CommandResult::RawOutput { stdout, .. } => stdout,
        other => panic!("Expected RawOutput, got: {other:?}"),
    };

    assert_eq!(stdout.trim_end(), "\"Hello\"");
}

/// Regression: the `run` path must surface the invocation's workspace and
/// conversation IDs to the tool command via `context.workspace_id` and
/// `context.conversation_id`.
/// A non-empty `InvocationContext` pins the wiring so the fields can't be
/// silently dropped or emptied.
#[tokio::test]
#[cfg(unix)]
async fn test_execute_local_exposes_invocation_ids_in_context() {
    use jp_config::{
        AppConfig, Config,
        conversation::tool::{PartialToolConfig, ToolConfig},
    };

    let partial: PartialToolConfig = serde_json::from_value(json!({
        "source": "local",
        "command": "echo {{context.workspace_id}}-{{context.conversation_id}}",
    }))
    .expect("valid partial tool config");
    let tool = ToolConfig::from_partial(partial, vec![]).expect("resolved tool config");

    let mut cfg = AppConfig::new_test();
    cfg.conversation.tools.insert("echo_ids".to_owned(), tool);
    let config = cfg
        .conversation
        .tools
        .get("echo_ids")
        .expect("tool present");

    let definition = ToolDefinition {
        name: "echo_ids".to_owned(),
        docs: ToolDocs::default(),
        parameters: IndexMap::new(),
    };
    let invocation = InvocationContext {
        workspace_id: "ws-abc".to_owned(),
        conversation_id: "conv-xyz".to_owned(),
    };
    let mcp_client = Client::new(IndexMap::new());
    let builtins = builtin::BuiltinExecutors::new();

    let outcome = definition
        .execute(
            "call-1".to_owned(),
            json!({}),
            &IndexMap::new(),
            &config,
            &mcp_client,
            Utf8Path::new("/tmp"),
            CancellationToken::new(),
            &builtins,
            None,
            &invocation,
        )
        .await
        .expect("execution succeeds");

    match outcome {
        ExecutionOutcome::Completed {
            result: Ok(out), ..
        } => assert!(
            out.contains("ws-abc-conv-xyz"),
            "expected workspace/conversation IDs in tool output, got: {out:?}"
        ),
        other => panic!("expected completed success, got: {other:?}"),
    }
}

/// Regression for RFD 081: `tool_definitions` keeps a *forced* tool that is
/// merely disabled (`OFF`), but always drops a locked-off tool (`state =
/// false`, `allow_toggle = never`) even when it is forced.
#[tokio::test]
async fn test_tool_definitions_forced_tool_drops_locked_off() {
    use jp_config::{
        AppConfig, Config,
        conversation::tool::{PartialToolConfig, ToolConfig},
    };

    let off: PartialToolConfig = serde_json::from_value(json!({
        "source": "local",
        "command": "echo off",
        "enable": false,
    }))
    .expect("valid partial tool config");
    let locked_off: PartialToolConfig = serde_json::from_value(json!({
        "source": "local",
        "command": "echo locked",
        "enable": { "state": false, "allow_toggle": "never" },
    }))
    .expect("valid partial tool config");

    let mut cfg = AppConfig::new_test();
    cfg.conversation.tools.insert(
        "off_tool".to_owned(),
        ToolConfig::from_partial(off, vec![]).expect("resolved tool config"),
    );
    cfg.conversation.tools.insert(
        "locked_off_tool".to_owned(),
        ToolConfig::from_partial(locked_off, vec![]).expect("resolved tool config"),
    );

    let mcp_client = Client::new(IndexMap::new());

    // Forcing the toggleable OFF tool keeps it in the definitions.
    let defs = tool_definitions(cfg.conversation.tools.iter(), &mcp_client, Some("off_tool"))
        .await
        .expect("tool definitions resolve");
    assert!(
        defs.iter().any(|d| d.name == "off_tool"),
        "a forced toggleable OFF tool must be kept"
    );

    // Forcing the locked-off tool still drops it.
    let defs = tool_definitions(
        cfg.conversation.tools.iter(),
        &mcp_client,
        Some("locked_off_tool"),
    )
    .await
    .expect("tool definitions resolve");
    assert!(
        !defs.iter().any(|d| d.name == "locked_off_tool"),
        "a locked-off tool must be dropped even when forced"
    );
}

mod merge_mcp_param {
    use indexmap::IndexMap;
    use jp_config::conversation::tool::{OneOrManyTypes, ToolParameterConfig};
    use serde_json::{Value, json};

    use super::super::{merge_mcp_param, merge_mcp_parameter};

    fn cfg(kind: &str) -> ToolParameterConfig {
        ToolParameterConfig {
            kind: Some(OneOrManyTypes::One(kind.to_owned())),
            default: None,
            required: None,
            summary: None,
            description: None,
            examples: None,
            enumeration: None,
            items: None,
            properties: IndexMap::default(),
        }
    }

    /// Regression for the `crate_search_items.kinds` case.
    /// The MCP schema declares `items: {"$ref": ...}` (no plain `type`), which
    /// our parser can't inline.
    /// The user's TOML override provides a usable `items` config and must win.
    #[test]
    fn user_items_override_used_when_mcp_items_lacks_type() {
        let opts = json!({
            "type": "array",
            "items": { "$ref": "#/$defs/EntryType" }
        });
        let override_cfg = ToolParameterConfig {
            items: Some(Box::new(cfg("string"))),
            ..cfg("array")
        };

        let merged = merge_mcp_param("kinds", &opts, Some(&override_cfg), false).unwrap();

        let items = merged.items.expect("items should be set from override");
        assert!(matches!(
            items.kind,
            OneOrManyTypes::One(ref s) if s == "string"
        ));
    }

    #[test]
    fn item_enum_override_preserves_mcp_item_type() {
        let opts = json!({
            "type": "array",
            "items": { "type": "string" }
        });
        let override_cfg: ToolParameterConfig = serde_json::from_value(json!({
            "items": {
                "enum": ["projects/jp", "task", "idea"]
            }
        }))
        .unwrap();

        let merged = merge_mcp_param("tags", &opts, Some(&override_cfg), false).unwrap();

        let items = merged.items.expect("items present");
        assert!(matches!(
            items.kind,
            OneOrManyTypes::One(ref s) if s == "string"
        ));
        assert_eq!(items.enumeration, vec![
            Value::from("projects/jp"),
            Value::from("task"),
            Value::from("idea")
        ]);
    }

    /// When the user provides no override, fall back to the MCP schema's
    /// `items` (existing behavior, preserved by the refactor).
    #[test]
    fn mcp_items_used_when_no_user_override() {
        let opts = json!({
            "type": "array",
            "items": { "type": "integer" }
        });

        let merged = merge_mcp_param("xs", &opts, None, false).unwrap();

        let items = merged.items.expect("items from MCP");
        assert!(matches!(
            items.kind,
            OneOrManyTypes::One(ref s) if s == "integer"
        ));
    }

    #[test]
    fn unresolved_mcp_item_reference_is_rejected() {
        let opts = json!({
            "type": "array",
            "items": { "$ref": "#/$defs/EntryType" }
        });

        let error = merge_mcp_param("kinds", &opts, None, false).unwrap_err();

        assert_eq!(
            error.to_string(),
            "Invalid schema at `parameters.kinds.items.type`: schema does not declare a supported \
             type"
        );
    }

    /// User `properties` override is honored, mirroring `items`.
    #[test]
    fn user_properties_override_is_used() {
        let opts = json!({ "type": "object" });
        let mut props = IndexMap::new();
        props.insert("name".to_owned(), cfg("string"));
        let override_cfg = ToolParameterConfig {
            properties: props,
            ..cfg("object")
        };

        let merged = merge_mcp_param("target", &opts, Some(&override_cfg), false).unwrap();

        assert_eq!(merged.properties.len(), 1);
        assert!(merged.properties.contains_key("name"));
    }

    /// Empty user properties don't override — falls back to default (currently
    /// empty, since MCP-side nested property resolution isn't implemented).
    #[test]
    fn empty_user_properties_falls_back_to_default() {
        let opts = json!({ "type": "object" });

        let merged = merge_mcp_param("target", &opts, None, false).unwrap();

        assert!(merged.properties.is_empty());
    }

    /// `required: false → true` allowed by override (tightening).
    #[test]
    fn override_can_tighten_required() {
        let opts = json!({ "type": "string" });
        let override_cfg = ToolParameterConfig {
            required: Some(true),
            ..cfg("string")
        };

        let merged = merge_mcp_param("x", &opts, Some(&override_cfg), false).unwrap();

        assert!(merged.required);
    }

    /// `required: true → false` ignored — the server's contract wins.
    #[test]
    fn override_cannot_loosen_required() {
        let opts = json!({ "type": "string" });
        let override_cfg = ToolParameterConfig {
            required: Some(false),
            ..cfg("string")
        };

        let merged = merge_mcp_param("x", &opts, Some(&override_cfg), true).unwrap();

        assert!(merged.required, "MCP-required field cannot be loosened");
    }

    #[test]
    fn user_cannot_change_mcp_parameter_type() {
        let opts = json!({ "type": "integer" });
        let override_cfg = cfg("string");

        let error = merge_mcp_param("count", &opts, Some(&override_cfg), false).unwrap_err();

        assert_eq!(
            error.to_string(),
            "Invalid schema at `parameters.count.type`: MCP declares integer, but the \
             configuration declares string"
        );
    }

    /// `enum` from MCP carries through when no user override.
    #[test]
    fn mcp_enum_used_when_no_user_enumeration() {
        let opts = json!({
            "type": "string",
            "enum": ["a", "b", "c"]
        });

        let merged = merge_mcp_param("x", &opts, None, false).unwrap();

        assert_eq!(merged.enumeration, vec![
            Value::from("a"),
            Value::from("b"),
            Value::from("c")
        ]);
    }

    #[test]
    fn unsupported_parameter_type_is_rejected() {
        let opts = json!({ "type": "strng" });

        let error = merge_mcp_param("name", &opts, None, false).unwrap_err();

        assert_eq!(
            error.to_string(),
            "Invalid schema at `parameters.name.type`: unsupported JSON type `strng`"
        );
    }

    #[test]
    fn malformed_mcp_enum_is_rejected() {
        let opts = json!({
            "type": "string",
            "enum": "task"
        });

        let error = merge_mcp_param("kind", &opts, None, false).unwrap_err();

        assert_eq!(
            error.to_string(),
            "Invalid schema at `parameters.kind.enum`: enum must be an array"
        );
    }

    /// Narrowing an inherited enum while inheriting the server's `default` is
    /// the realistic way to end up with a default the enum forbids.
    #[test]
    fn default_outside_a_narrowed_enum_is_rejected() {
        let opts = json!({
            "type": "string",
            "enum": ["all", "open", "closed"],
            "default": "all"
        });
        let override_cfg = ToolParameterConfig {
            enumeration: Some(vec![Value::from("open"), Value::from("closed")]),
            ..cfg("string")
        };

        let error = merge_mcp_param("state", &opts, Some(&override_cfg), false).unwrap_err();

        assert_eq!(
            error.to_string(),
            "Invalid schema at `parameters.state.default`: default value \"all\" is not allowed \
             by the enum"
        );
    }

    #[test]
    fn default_outside_a_property_enum_is_rejected() {
        let opts = json!({
            "type": "object",
            "default": { "mode": "fast" },
            "properties": {
                "mode": { "type": "string", "enum": ["safe"] }
            }
        });

        let error = merge_mcp_param("target", &opts, None, false).unwrap_err();

        assert_eq!(
            error.to_string(),
            "Invalid schema at `parameters.target.default.mode`: default value \"fast\" is not \
             allowed by the enum"
        );
    }

    /// JSON Schema type arrays are unordered, so restating an inherited union
    /// in a different order describes the same type set.
    #[test]
    fn override_may_reorder_an_inherited_union() {
        let opts = json!({ "type": ["string", "null"] });
        let override_cfg = ToolParameterConfig {
            kind: Some(OneOrManyTypes::Many(vec![
                "null".to_owned(),
                "string".to_owned(),
            ])),
            ..cfg("string")
        };

        let merged = merge_mcp_param("content", &opts, Some(&override_cfg), false).unwrap();

        assert_eq!(
            merged.kind,
            OneOrManyTypes::Many(vec!["string".to_owned(), "null".to_owned()])
        );
    }

    #[test]
    fn override_may_restate_a_scalar_type_as_a_single_element_array() {
        let opts = json!({ "type": "string" });
        let override_cfg = ToolParameterConfig {
            kind: Some(OneOrManyTypes::Many(vec!["string".to_owned()])),
            ..cfg("string")
        };

        let merged = merge_mcp_param("name", &opts, Some(&override_cfg), false).unwrap();

        assert_eq!(merged.kind, OneOrManyTypes::One("string".to_owned()));
    }

    /// The resolved schema keeps the server's type declaration, so a malformed
    /// override type list has to be reported against the override itself.
    #[test]
    fn duplicate_type_in_an_override_is_rejected() {
        let opts = json!({ "type": "string" });
        let override_cfg = ToolParameterConfig {
            kind: Some(OneOrManyTypes::Many(vec![
                "string".to_owned(),
                "string".to_owned(),
            ])),
            ..cfg("string")
        };

        let error = merge_mcp_param("name", &opts, Some(&override_cfg), false).unwrap_err();

        assert_eq!(
            error.to_string(),
            "Invalid schema at `parameters.name.type`: type values must be unique; duplicate type \
             `string`"
        );
    }

    /// A known fidelity gap: `anyOf` variants that declare no JSON type (such
    /// as a `$ref` to a `$defs` entry) are dropped, so `Option<Struct>`
    /// resolves to null-only.
    /// The drop is logged; resolving the reference needs the tool's root schema
    /// and is tracked separately.
    #[test]
    fn optional_reference_variant_narrows_to_null() {
        let opts = json!({
            "anyOf": [
                { "$ref": "#/$defs/Options" },
                { "type": "null" }
            ]
        });

        let merged = merge_mcp_param("options", &opts, None, false).unwrap();

        assert_eq!(merged.kind, OneOrManyTypes::Many(vec!["null".to_owned()]));
    }

    #[test]
    fn default_values_must_match_nested_item_schema() {
        let opts = json!({
            "type": "array",
            "default": ["task", 1],
            "items": { "type": "string" }
        });

        let error = merge_mcp_param("tags", &opts, None, false).unwrap_err();

        assert_eq!(
            error.to_string(),
            "Invalid schema at `parameters.tags.default[1]`: default value 1 has type integer, \
             but the schema requires string"
        );
    }

    #[test]
    fn duplicate_enum_values_are_rejected() {
        let opts = json!({ "type": "string" });
        let override_cfg = ToolParameterConfig {
            enumeration: Some(vec![Value::from("task"), Value::from("task")]),
            ..cfg("string")
        };

        let error = merge_mcp_param("kind", &opts, Some(&override_cfg), false).unwrap_err();

        assert_eq!(
            error.to_string(),
            "Invalid schema at `parameters.kind.enum`: enum values must be unique; duplicate \
             value \"task\""
        );
    }

    #[test]
    fn complete_array_enum_values_must_match_item_schema() {
        let opts = json!({
            "type": "array",
            "items": { "type": "string" }
        });
        let override_cfg = ToolParameterConfig {
            enumeration: Some(vec![json!(["task", 1])]),
            ..cfg("array")
        };

        let error = merge_mcp_param("tags", &opts, Some(&override_cfg), false).unwrap_err();

        assert_eq!(
            error.to_string(),
            "Invalid schema at `parameters.tags.enum[0][1]`: enum value 1 has type integer, but \
             the schema requires string"
        );
    }

    #[test]
    fn array_without_items_is_rejected() {
        let opts = json!({ "type": "array" });

        let error = merge_mcp_param("tags", &opts, None, false).unwrap_err();

        assert_eq!(
            error.to_string(),
            "Invalid schema at `parameters.tags.items`: array schemas must declare an item schema"
        );
    }

    #[test]
    fn schema_errors_retain_the_tool_path() {
        let source = json!({
            "type": "array",
            "items": { "type": "string" }
        });
        let override_config = ToolParameterConfig {
            enumeration: Some(vec![Value::from("projects/jp")]),
            ..cfg("array")
        };

        let error = merge_mcp_parameter(
            "conversation.tools.bear_note_create.parameters.tags",
            &source,
            Some(&override_config),
            false,
        )
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "Invalid schema at `conversation.tools.bear_note_create.parameters.tags.enum`: enum \
             value \"projects/jp\" has type string, but the schema requires array; use \
             `conversation.tools.bear_note_create.parameters.tags.items.enum` to constrain array \
             elements"
        );
    }

    #[test]
    fn scalar_enum_on_array_parameter_is_rejected() {
        let opts = json!({
            "type": "array",
            "items": { "type": "string" }
        });
        let override_cfg = ToolParameterConfig {
            enumeration: Some(vec![
                Value::from("projects/jp"),
                Value::from("task"),
                Value::from("idea"),
            ]),
            ..cfg("array")
        };

        let error = merge_mcp_param("tags", &opts, Some(&override_cfg), false).unwrap_err();

        assert_eq!(
            error.to_string(),
            "Invalid schema at `parameters.tags.enum`: enum value \"projects/jp\" has type \
             string, but the schema requires array; use `parameters.tags.items.enum` to constrain \
             array elements"
        );
    }
}
