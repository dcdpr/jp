use serde_json::json;

use super::*;

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

/// A schema node of the given type, carrying a default value.
fn param_with_default(kind: &str, default: &Value) -> Value {
    json!({ "type": kind, "default": default })
}

fn definition(parameters: Value) -> ToolDefinition {
    ToolDefinition {
        name: "test".to_owned(),
        docs: ToolDocs::default(),
        parameters,
    }
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

    definition(parameters).coerce_arguments(&mut arguments);

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

    definition(parameters).coerce_arguments(&mut arguments);

    assert_eq!(Value::Object(arguments), json!({ "value": "3" }));
}

/// A property with an `enum` and no `type` still says what it takes: the string
/// the model sent is not a member, and the number it parses to is.
#[test]
fn coerces_a_string_the_enum_excludes_into_the_member_it_parses_to() {
    let parameters = schema([("value", json!({ "enum": [3] }), false)]);
    let mut arguments = json!({ "value": "3" }).as_object().cloned().unwrap();

    definition(parameters).coerce_arguments(&mut arguments);

    assert_eq!(Value::Object(arguments), json!({ "value": 3 }));
}

/// The mirror case: the enum lists the string itself, so parsing it would
/// produce the one value the schema forbids.
#[test]
fn leaves_a_string_alone_when_the_enum_lists_it() {
    let parameters = schema([("value", json!({ "enum": ["3"] }), false)]);
    let mut arguments = json!({ "value": "3" }).as_object().cloned().unwrap();

    definition(parameters).coerce_arguments(&mut arguments);

    assert_eq!(Value::Object(arguments), json!({ "value": "3" }));
}

#[test]
fn test_validate_tool_arguments() {
    struct TestCase {
        arguments: Map<String, Value>,
        parameters: Value,
        want: Result<(), Error>,
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
            want: Err(Error::Arguments {
                missing: vec!["foo".to_owned()],
                unknown: vec![],
            }),
        }),
        ("unknown", TestCase {
            arguments: Map::from_iter([("foo".to_owned(), json!("bar"))]),
            parameters: schema([("bar", param("string"), false)]),
            want: Err(Error::Arguments {
                missing: vec![],
                unknown: vec!["foo".to_owned()],
            }),
        }),
        ("both", TestCase {
            arguments: Map::from_iter([("foo".to_owned(), json!("bar"))]),
            parameters: schema([("bar", param("string"), true)]),
            want: Err(Error::Arguments {
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
        Err(Error::Arguments {
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
        Err(Error::Arguments {
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
    let Error::Arguments { missing, unknown } = err.unwrap_err() else {
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
        Err(Error::Arguments {
            missing: vec![],
            unknown: vec!["bogus".to_owned()],
        })
    );

    // Invalid: missing required field inside the object.
    let args = json!({ "name": "test", "config": { "verbose": true } });
    assert_eq!(
        validate_tool_arguments(args.as_object().unwrap(), &parameters),
        Err(Error::Arguments {
            missing: vec!["output".to_owned()],
            unknown: vec![],
        })
    );
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
