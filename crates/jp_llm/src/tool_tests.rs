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
    let question = Question::text("q1", "What is your name?").unwrap();

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
        question: Question::text("q", "?").unwrap(),
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
        question: Question::boolean("q", "?").unwrap(),
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

#[test]
fn parse_command_output_valid_needs_input() {
    let stdout = br#"{"type":"needs_input","question":{"id":"confirm","text":"?","pre_amble":null,"answer_type":{"type":"boolean"},"default":null}}"#;
    assert!(matches!(
        parse_command_output(stdout, b"", true),
        CommandResult::NeedsInput(_)
    ));
}

#[test]
fn parse_command_output_dotted_question_id_is_invalid_inquiry() {
    let stdout = br#"{"type":"needs_input","question":{"id":"a.b","text":"?","pre_amble":null,"answer_type":{"type":"boolean"},"default":null}}"#;
    let result = parse_command_output(stdout, b"", true);
    assert!(matches!(
        result,
        CommandResult::InvalidInquiry { ref question_id } if question_id == "a.b"
    ));
    // Renders as a tool-level error, not raw text.
    assert!(result.into_tool_result("t").is_err());
}

#[test]
fn parse_command_output_empty_question_id_is_invalid_inquiry() {
    let stdout = br#"{"type":"needs_input","question":{"id":"","text":"?","pre_amble":null,"answer_type":{"type":"boolean"},"default":null}}"#;
    let result = parse_command_output(stdout, b"", true);
    assert!(matches!(
        result,
        CommandResult::InvalidInquiry { ref question_id } if question_id.is_empty()
    ));
    assert!(result.into_tool_result("t").is_err());
}

#[test]
fn parse_command_output_legacy_answer_type_shape_is_malformed_inquiry() {
    // A stale local-tool binary emits the pre-082 externally-tagged answer
    // type (`"answer_type":"Boolean"`) instead of the internally-tagged
    // `{"type":"boolean"}` this build parses. The question id is valid, so
    // the payload must surface as a tool-level error rather than being handed
    // to the model as raw JSON.
    let stdout = br#"{"type":"needs_input","question":{"id":"apply_changes","text":"Apply?","answer_type":"Boolean","default":true}}"#;
    let result = parse_command_output(stdout, b"", true);
    assert!(
        matches!(result, CommandResult::MalformedInquiry { .. }),
        "expected MalformedInquiry, got {result:?}"
    );
    // Renders as a tool-level error, not raw text.
    assert!(result.into_tool_result("fs_modify_file").is_err());
}

#[test]
fn parse_command_output_needs_input_missing_field_is_malformed_inquiry() {
    // A `needs_input` missing a required question field fails to deserialize;
    // with a valid id it is a malformed inquiry, not raw output.
    let stdout = br#"{"type":"needs_input","question":{"id":"confirm"}}"#;
    let result = parse_command_output(stdout, b"", true);
    assert!(
        matches!(result, CommandResult::MalformedInquiry { .. }),
        "expected MalformedInquiry, got {result:?}"
    );
    assert!(result.into_tool_result("t").is_err());
}

#[test]
fn parse_command_output_non_outcome_is_raw() {
    assert!(matches!(
        parse_command_output(b"plain text", b"", true),
        CommandResult::RawOutput { .. }
    ));
}

#[test]
fn parse_command_output_non_needs_input_json_is_raw() {
    // Valid JSON that is not an `Outcome` and not a `needs_input` payload
    // stays raw output — the malformed-inquiry path must not swallow it.
    let stdout = br#"{"some":"object","the_tool":"did not use the protocol"}"#;
    assert!(matches!(
        parse_command_output(stdout, b"", true),
        CommandResult::RawOutput { .. }
    ));
}

/// Build a parameters schema from `(name, node, required)` triples.
fn schema<const N: usize>(properties: [(&str, Value, bool); N]) -> Value {
    let required = properties
        .iter()
        .filter(|(_, _, required)| *required)
        .map(|(name, _, _)| Value::String((*name).to_owned()))
        .collect::<Vec<_>>();
    let properties = properties
        .into_iter()
        .map(|(name, node, _)| (name.to_owned(), node))
        .collect::<Map<_, _>>();

    json!({ "type": "object", "properties": properties, "required": required })
}

/// A schema node of the given type.
fn param(kind: &str) -> Value {
    json!({ "type": kind })
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
    let parameters = schema([
        ("path", param("string"), true),
        ("start_line", param("integer"), false),
        ("enabled", param("boolean"), false),
        (
            "string_or_integer",
            json!({ "type": ["string", "integer"] }),
            false,
        ),
        (
            "patterns",
            json!({
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": { "count": { "type": "integer" } },
                    "required": ["count"]
                }
            }),
            false,
        ),
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

/// Coercion repairs a string the schema cannot accept.
/// A parameter that declares no type accepts the string as written, so a
/// JSON-looking string reaches the tool as the text the model sent.
#[test]
fn leaves_strings_alone_for_a_parameter_with_no_declared_type() {
    let parameters = schema([("value", json!({ "description": "Any JSON value." }), false)]);
    let mut arguments = json!({ "value": "3" }).as_object().cloned().unwrap();

    ToolDefinition {
        name: "test".to_owned(),
        docs: ToolDocs::default(),
        parameters,
    }
    .coerce_arguments(&mut arguments);

    assert_eq!(Value::Object(arguments), json!({ "value": "3" }));
}

/// A property with an `enum` and no `type` still says what it takes: the string
/// the model sent is not a member, and the number it parses to is.
#[test]
fn coerces_a_string_the_enum_excludes_into_the_member_it_parses_to() {
    let parameters = schema([("value", json!({ "enum": [3] }), false)]);
    let mut arguments = json!({ "value": "3" }).as_object().cloned().unwrap();

    ToolDefinition {
        name: "test".to_owned(),
        docs: ToolDocs::default(),
        parameters,
    }
    .coerce_arguments(&mut arguments);

    assert_eq!(Value::Object(arguments), json!({ "value": 3 }));
}

/// The mirror case: the enum lists the string itself, so parsing it would
/// produce the one value the schema forbids.
#[test]
fn leaves_a_string_alone_when_the_enum_lists_it() {
    let parameters = schema([("value", json!({ "enum": ["3"] }), false)]);
    let mut arguments = json!({ "value": "3" }).as_object().cloned().unwrap();

    ToolDefinition {
        name: "test".to_owned(),
        docs: ToolDocs::default(),
        parameters,
    }
    .coerce_arguments(&mut arguments);

    assert_eq!(Value::Object(arguments), json!({ "value": "3" }));
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
        parameters: schema([("start_line", param("integer"), false)]),
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
        parameters: Value,
        want: Result<(), ToolError>,
    }

    let cases = vec![
        ("empty", TestCase {
            arguments: Map::new(),
            parameters: schema([]),
            want: Ok(()),
        }),
        ("correct", TestCase {
            arguments: Map::from_iter([("foo".to_owned(), json!("bar"))]),
            parameters: schema([
                ("foo", param("string"), true),
                ("bar", param("string"), false),
            ]),
            want: Ok(()),
        }),
        ("missing", TestCase {
            arguments: Map::new(),
            parameters: schema([("foo", param("string"), true)]),
            want: Err(ToolError::Arguments {
                missing: vec!["foo".to_owned()],
                unknown: vec![],
            }),
        }),
        ("unknown", TestCase {
            arguments: Map::from_iter([("foo".to_owned(), json!("bar"))]),
            parameters: schema([("bar", param("string"), false)]),
            want: Err(ToolError::Arguments {
                missing: vec![],
                unknown: vec!["foo".to_owned()],
            }),
        }),
        ("both", TestCase {
            arguments: Map::from_iter([("foo".to_owned(), json!("bar"))]),
            parameters: schema([("bar", param("string"), true)]),
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
    let parameters = schema([
        ("path", param("string"), true),
        (
            "patterns",
            json!({
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "old": { "type": "string" },
                        "new": { "type": "string" }
                    },
                    "required": ["old", "new"]
                }
            }),
            true,
        ),
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
    let parameters = schema([
        ("name", param("string"), true),
        (
            "config",
            json!({
                "type": "object",
                "properties": {
                    "verbose": { "type": "boolean" },
                    "output": { "type": "string" }
                },
                "required": ["output"]
            }),
            false,
        ),
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

/// A schema node of the given type, carrying a default value.
fn param_with_default(kind: &str, default: &Value) -> Value {
    json!({ "type": kind, "default": default })
}

#[test]
fn test_apply_defaults_fills_missing_required_with_default() {
    let parameters = schema([
        ("path", param("string"), true),
        (
            "use_regex",
            param_with_default("boolean", &json!(false)),
            true,
        ),
    ]);

    let mut args: Map<String, Value> = Map::from_iter([("path".to_owned(), json!("src/lib.rs"))]);

    apply_parameter_defaults(&mut args, &parameters);

    assert_eq!(args.get("path"), Some(&json!("src/lib.rs")));
    assert_eq!(args.get("use_regex"), Some(&json!(false)));
}

#[test]
fn test_apply_defaults_does_not_overwrite_provided_values() {
    let parameters = schema([(
        "use_regex",
        param_with_default("boolean", &json!(false)),
        true,
    )]);

    let mut args: Map<String, Value> = Map::from_iter([("use_regex".to_owned(), json!(true))]);

    apply_parameter_defaults(&mut args, &parameters);

    assert_eq!(args.get("use_regex"), Some(&json!(true)));
}

#[test]
fn test_apply_defaults_fills_optional_param_with_default() {
    let parameters = schema([(
        "verbose",
        param_with_default("boolean", &json!(false)),
        false,
    )]);

    let mut args: Map<String, Value> = Map::new();
    apply_parameter_defaults(&mut args, &parameters);

    assert_eq!(args.get("verbose"), Some(&json!(false)));
}

#[test]
fn test_apply_defaults_skips_params_without_default() {
    let parameters = schema([("path", param("string"), true)]);

    let mut args: Map<String, Value> = Map::new();
    apply_parameter_defaults(&mut args, &parameters);

    assert!(!args.contains_key("path"));
}

#[test]
fn test_apply_defaults_recurses_into_objects() {
    let parameters = schema([(
        "config",
        json!({
            "type": "object",
            "properties": { "verbose": { "type": "boolean", "default": true } }
        }),
        false,
    )]);

    let mut args: Map<String, Value> = Map::from_iter([("config".to_owned(), json!({}))]);

    apply_parameter_defaults(&mut args, &parameters);

    assert_eq!(args["config"]["verbose"], json!(true));
}

#[test]
fn test_apply_defaults_recurses_into_array_items() {
    let parameters = schema([(
        "items",
        json!({
            "type": "array",
            "items": {
                "type": "object",
                "properties": { "enabled": { "type": "boolean", "default": true } }
            }
        }),
        true,
    )]);

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
    let parameters = schema([
        ("path", param("string"), true),
        (
            "replace_using_regex",
            param_with_default("boolean", &json!(false)),
            true,
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
        parameters: schema([]),
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

/// A built-in that reports it ran, so dispatch can be observed.
struct ReachedBuiltin;

#[async_trait::async_trait]
impl builtin::BuiltinTool for ReachedBuiltin {
    async fn execute(&self, _: &Value, _: &IndexMap<String, Value>) -> jp_tool::Outcome {
        "reached".into()
    }
}

/// A built-in tool may be keyed differently from the implementation it names:
/// `source = "builtin.describe_tools"` under a `docs` key.
/// Dispatch keys on the source's tool name, matching how the local and MCP
/// paths treat theirs.
#[tokio::test]
async fn test_execute_builtin_dispatches_on_source_name() {
    use jp_config::{
        AppConfig, Config,
        conversation::tool::{PartialToolConfig, ToolConfig},
    };

    let partial: PartialToolConfig = serde_json::from_value(json!({
        "source": "builtin.describe_tools",
    }))
    .expect("valid partial tool config");
    let tool = ToolConfig::from_partial(partial, vec![]).expect("resolved tool config");

    let mut cfg = AppConfig::new_test();
    cfg.conversation.tools.insert("docs".to_owned(), tool);
    let config = cfg.conversation.tools.get("docs").expect("tool present");

    let definition = ToolDefinition {
        name: "docs".to_owned(),
        docs: ToolDocs::default(),
        parameters: schema([]),
    };
    let mcp_client = Client::new(IndexMap::new());
    let builtins = builtin::BuiltinExecutors::new().register("describe_tools", ReachedBuiltin);

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
            &InvocationContext::default(),
        )
        .await
        .expect("execution succeeds");

    match outcome {
        ExecutionOutcome::Completed {
            result: Ok(out), ..
        } => assert_eq!(out, "reached"),
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

/// A tool whose schema cannot be resolved is dropped from the request rather
/// than failing the whole query, mirroring how an unavailable MCP server is
/// handled.
#[tokio::test]
async fn tool_with_an_unresolvable_schema_is_skipped() {
    let broken: PartialToolConfig = serde_json::from_value(json!({
        "source": "local",
        "command": "echo broken",
        "parameters": { "tags": { "type": "array" } },
    }))
    .expect("valid partial tool config");
    let healthy: PartialToolConfig = serde_json::from_value(json!({
        "source": "local",
        "command": "echo fine",
        "parameters": { "path": { "type": "string" } },
    }))
    .expect("valid partial tool config");

    let mut cfg = AppConfig::new_test();
    cfg.conversation.tools.insert(
        "broken_tool".to_owned(),
        ToolConfig::from_partial(broken, vec![]).expect("resolved tool config"),
    );
    cfg.conversation.tools.insert(
        "healthy_tool".to_owned(),
        ToolConfig::from_partial(healthy, vec![]).expect("resolved tool config"),
    );

    let defs = tool_definitions(
        cfg.conversation.tools.iter(),
        &Client::new(IndexMap::new()),
        None,
    )
    .await
    .expect("a broken tool must not fail the query");

    let names = defs.iter().map(|d| d.name.as_str()).collect::<Vec<_>>();
    assert_eq!(names, vec!["healthy_tool"]);
}

/// Naming a tool with `--tool` is an explicit request for it, so its schema
/// error surfaces instead of the tool silently disappearing.
#[tokio::test]
async fn forced_tool_with_an_unresolvable_schema_still_errors() {
    let broken: PartialToolConfig = serde_json::from_value(json!({
        "source": "local",
        "command": "echo broken",
        "parameters": { "tags": { "type": "array" } },
    }))
    .expect("valid partial tool config");

    let mut cfg = AppConfig::new_test();
    cfg.conversation.tools.insert(
        "broken_tool".to_owned(),
        ToolConfig::from_partial(broken, vec![]).expect("resolved tool config"),
    );

    let error = tool_definitions(
        cfg.conversation.tools.iter(),
        &Client::new(IndexMap::new()),
        Some("broken_tool"),
    )
    .await
    .unwrap_err();

    assert_eq!(
        error.to_string(),
        "Invalid schema at `conversation.tools.broken_tool.parameters.tags.items`: array schemas \
         must declare an item schema"
    );
}
