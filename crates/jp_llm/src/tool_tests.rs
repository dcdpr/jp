use jp_tool::AnswerType;

use super::*;

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
    let question = Question {
        id: "q1".to_string(),
        text: "What is your name?".to_string(),
        answer_type: AnswerType::Text,
        default: None,
    };

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
        question: Question {
            id: "q".to_string(),
            text: "?".to_string(),
            answer_type: AnswerType::Text,
            default: None,
        },
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
        question: Question {
            id: "q".to_string(),
            text: "?".to_string(),
            answer_type: AnswerType::Boolean,
            default: None,
        },
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

/// Build a minimal `ToolParameterConfig` for use in validation tests.
fn param(kind: &str, required: bool) -> ToolParameterConfig {
    ToolParameterConfig {
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

#[test]
fn test_validate_tool_arguments() {
    struct TestCase {
        arguments: Map<String, Value>,
        parameters: IndexMap<String, ToolParameterConfig>,
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
        ("patterns".to_owned(), ToolParameterConfig {
            kind: "array".to_owned().into(),
            required: true,
            items: Some(Box::new(ToolParameterConfig {
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
        ("config".to_owned(), ToolParameterConfig {
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
fn param_with_default(kind: &str, required: bool, default: Value) -> ToolParameterConfig {
    ToolParameterConfig {
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
    let parameters = IndexMap::from_iter([("config".to_owned(), ToolParameterConfig {
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
    let parameters = IndexMap::from_iter([("items".to_owned(), ToolParameterConfig {
        kind: "array".to_owned().into(),
        required: true,
        items: Some(Box::new(ToolParameterConfig {
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
