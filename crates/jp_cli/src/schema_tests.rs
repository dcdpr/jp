use serde_json::json;

use super::*;

#[test]
fn single_string_field() {
    let schema = parse_schema_dsl("summary").unwrap();
    assert_eq!(
        schema,
        json!({
            "type": "object",
            "properties": {
                "summary": { "type": "string" }
            },
            "required": ["summary"]
        })
    );
}

#[test]
fn multiple_string_fields() {
    let schema = parse_schema_dsl("name, bio").unwrap();
    assert_eq!(
        schema,
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "bio": { "type": "string" }
            },
            "required": ["name", "bio"]
        })
    );
}

#[test]
fn field_with_int_type() {
    let schema = parse_schema_dsl("age int").unwrap();
    assert_eq!(
        schema,
        json!({
            "type": "object",
            "properties": {
                "age": { "type": "integer" }
            },
            "required": ["age"]
        })
    );
}

#[test]
fn field_with_float_type() {
    let schema = parse_schema_dsl("score float").unwrap();
    assert_eq!(
        schema,
        json!({
            "type": "object",
            "properties": {
                "score": { "type": "number" }
            },
            "required": ["score"]
        })
    );
}

#[test]
fn field_with_bool_type() {
    let schema = parse_schema_dsl("active bool").unwrap();
    assert_eq!(
        schema,
        json!({
            "type": "object",
            "properties": {
                "active": { "type": "boolean" }
            },
            "required": ["active"]
        })
    );
}

#[test]
fn field_with_description() {
    let schema = parse_schema_dsl("summary: a brief two-sentence summary").unwrap();
    assert_eq!(
        schema,
        json!({
            "type": "object",
            "properties": {
                "summary": {
                    "type": "string",
                    "description": "a brief two-sentence summary"
                }
            },
            "required": ["summary"]
        })
    );
}

#[test]
fn typed_field_with_description() {
    let schema = parse_schema_dsl("age int: the person's age in years").unwrap();
    assert_eq!(
        schema,
        json!({
            "type": "object",
            "properties": {
                "age": {
                    "type": "integer",
                    "description": "the person's age in years"
                }
            },
            "required": ["age"]
        })
    );
}

#[test]
fn mixed_fields() {
    let schema = parse_schema_dsl("name, age int, active bool, score float").unwrap();
    assert_eq!(
        schema,
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "age": { "type": "integer" },
                "active": { "type": "boolean" },
                "score": { "type": "number" }
            },
            "required": ["name", "age", "active", "score"]
        })
    );
}

#[test]
fn newline_separated_fields() {
    let input = "name: the person's name\nage int: their age\nbio: a short bio";
    let schema = parse_schema_dsl(input).unwrap();
    assert_eq!(
        schema,
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "the person's name" },
                "age": { "type": "integer", "description": "their age" },
                "bio": { "type": "string", "description": "a short bio" }
            },
            "required": ["name", "age", "bio"]
        })
    );
}

#[test]
fn newline_separated_with_commas_in_quoted_description() {
    let input = "name\nbio: \"a short bio, no more than three sentences\"";
    let schema = parse_schema_dsl(input).unwrap();
    assert_eq!(
        schema["properties"]["bio"]["description"],
        "a short bio, no more than three sentences"
    );
}

#[test]
fn json_passthrough() {
    let input = r#"{"type":"object","properties":{"x":{"type":"string"}}}"#;
    let schema = parse_schema_dsl(input).unwrap();
    assert_eq!(
        schema,
        json!({
            "type": "object",
            "properties": { "x": { "type": "string" } }
        })
    );
}

#[test]
fn whitespace_handling() {
    let schema = parse_schema_dsl("  name  ,  age int  ").unwrap();
    assert_eq!(
        schema,
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "age": { "type": "integer" }
            },
            "required": ["name", "age"]
        })
    );
}

#[test]
fn long_type_aliases() {
    let schema = parse_schema_dsl("a string, b integer, c number, d boolean").unwrap();
    assert_eq!(schema["properties"]["a"]["type"], "string");
    assert_eq!(schema["properties"]["b"]["type"], "integer");
    assert_eq!(schema["properties"]["c"]["type"], "number");
    assert_eq!(schema["properties"]["d"]["type"], "boolean");
}

#[test]
fn trailing_comma_ignored() {
    let schema = parse_schema_dsl("name, age int,").unwrap();
    assert_eq!(schema["required"], json!(["name", "age"]));
}

#[test]
fn field_with_hyphens_and_underscores() {
    let schema = parse_schema_dsl("first_name, last-name").unwrap();
    assert_eq!(schema["properties"]["first_name"]["type"], "string");
    assert_eq!(schema["properties"]["last-name"]["type"], "string");
}

#[test]
fn description_with_colons() {
    let schema = parse_schema_dsl("time: format: HH:MM:SS").unwrap();
    assert_eq!(
        schema["properties"]["time"]["description"],
        "format: HH:MM:SS"
    );
}

#[test]
fn empty_input_is_error() {
    assert!(matches!(parse_schema_dsl(""), Err(ParseError::Empty)));
    assert!(matches!(parse_schema_dsl("   "), Err(ParseError::Empty)));
}

#[test]
fn unknown_type_is_error() {
    let err = parse_schema_dsl("name, age blorp").unwrap_err();
    assert!(matches!(err, ParseError::UnknownType { ref given, .. } if given == "blorp"));
}

#[test]
fn dotted_field_name() {
    let schema = parse_schema_dsl("foo.bar").unwrap();
    assert_eq!(schema["properties"]["foo.bar"]["type"], "string");
}

#[test]
fn slash_in_field_name() {
    let schema = parse_schema_dsl("api/version").unwrap();
    assert_eq!(schema["properties"]["api/version"]["type"], "string");
}

#[test]
fn quoted_field_name() {
    let schema = parse_schema_dsl(r#""my field" int, name"#).unwrap();
    assert_eq!(schema["properties"]["my field"]["type"], "integer");
    assert_eq!(schema["properties"]["name"]["type"], "string");
}

#[test]
fn quoted_field_name_with_reserved_chars() {
    let schema = parse_schema_dsl(r#""items[0]" int"#).unwrap();
    assert_eq!(schema["properties"]["items[0]"]["type"], "integer");
}

#[test]
fn invalid_json_passthrough_is_error() {
    let err = parse_schema_dsl("{not valid json}").unwrap_err();
    assert!(matches!(err, ParseError::Json { .. }));
}

#[test]
fn optional_field() {
    let schema = parse_schema_dsl("name, ?nickname").unwrap();
    assert_eq!(
        schema,
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "nickname": { "type": "string" }
            },
            "required": ["name"]
        })
    );
}

#[test]
fn all_optional_omits_required() {
    let schema = parse_schema_dsl("?name, ?age int").unwrap();
    assert_eq!(
        schema,
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "age": { "type": "integer" }
            }
        })
    );
}

#[test]
fn optional_with_description() {
    let schema = parse_schema_dsl("?bio: a short biography").unwrap();
    assert_eq!(
        schema,
        json!({
            "type": "object",
            "properties": {
                "bio": { "type": "string", "description": "a short biography" }
            }
        })
    );
}

#[test]
fn any_type() {
    let schema = parse_schema_dsl("data any").unwrap();
    assert_eq!(
        schema,
        json!({
            "type": "object",
            "properties": {
                "data": {}
            },
            "required": ["data"]
        })
    );
}

#[test]
fn array_of_strings() {
    let schema = parse_schema_dsl("names [string]").unwrap();
    assert_eq!(
        schema,
        json!({
            "type": "object",
            "properties": {
                "names": { "type": "array", "items": { "type": "string" } }
            },
            "required": ["names"]
        })
    );
}

#[test]
fn array_of_ints() {
    let schema = parse_schema_dsl("scores [int]").unwrap();
    assert_eq!(
        schema["properties"]["scores"],
        json!({
            "type": "array",
            "items": { "type": "integer" }
        })
    );
}

#[test]
fn array_of_any() {
    let schema = parse_schema_dsl("items [any]").unwrap();
    assert_eq!(
        schema["properties"]["items"],
        json!({
            "type": "array",
            "items": {}
        })
    );
}

#[test]
fn bare_array_is_array_of_any() {
    let schema = parse_schema_dsl("items []").unwrap();
    assert_eq!(
        schema["properties"]["items"],
        json!({
            "type": "array",
            "items": {}
        })
    );
}

#[test]
fn array_with_description() {
    let schema = parse_schema_dsl("tags [string]: list of tags").unwrap();
    assert_eq!(
        schema["properties"]["tags"],
        json!({
            "type": "array",
            "items": { "type": "string" },
            "description": "list of tags"
        })
    );
}

#[test]
fn array_with_union_items() {
    let schema = parse_schema_dsl("data [string|int]").unwrap();
    assert_eq!(
        schema["properties"]["data"],
        json!({
            "type": "array",
            "items": {
                "anyOf": [{ "type": "string" }, { "type": "integer" }]
            }
        })
    );
}

#[test]
fn field_level_union() {
    let schema = parse_schema_dsl("foo [string]|int").unwrap();
    assert_eq!(
        schema["properties"]["foo"],
        json!({
            "anyOf": [
                { "type": "array", "items": { "type": "string" } },
                { "type": "integer" }
            ]
        })
    );
}

#[test]
fn nested_object() {
    let schema = parse_schema_dsl("address { city, zip }").unwrap();
    assert_eq!(
        schema,
        json!({
            "type": "object",
            "properties": {
                "address": {
                    "type": "object",
                    "properties": {
                        "city": { "type": "string" },
                        "zip": { "type": "string" }
                    },
                    "required": ["city", "zip"]
                }
            },
            "required": ["address"]
        })
    );
}

#[test]
fn nested_object_with_optional_fields() {
    let schema = parse_schema_dsl("address { city, ?zip }").unwrap();
    let addr = &schema["properties"]["address"];
    assert_eq!(addr["required"], json!(["city"]));
}

#[test]
fn nested_object_with_description() {
    let schema = parse_schema_dsl("address { city, zip }: the mailing address").unwrap();
    assert_eq!(
        schema["properties"]["address"]["description"],
        "the mailing address"
    );
}

#[test]
fn array_of_objects() {
    let schema = parse_schema_dsl("people [{ name, ?age int }]").unwrap();
    assert_eq!(
        schema["properties"]["people"],
        json!({
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "age": { "type": "integer" }
                },
                "required": ["name"]
            }
        })
    );
}

#[test]
fn deeply_nested_object() {
    let schema = parse_schema_dsl("a { b { c } }").unwrap();
    assert_eq!(
        schema["properties"]["a"]["properties"]["b"]["properties"]["c"]["type"],
        "string"
    );
}

#[test]
fn empty_object_is_error() {
    let err = parse_schema_dsl("data {}").unwrap_err();
    assert!(matches!(err, ParseError::EmptyObject { .. }));
}

#[test]
fn quoted_description() {
    let schema = parse_schema_dsl(r#"bar bool: "hello, universe""#).unwrap();
    assert_eq!(
        schema["properties"]["bar"]["description"],
        "hello, universe"
    );
}

#[test]
fn heredoc_description() {
    let schema = parse_schema_dsl("baz: \"\"\"\na longer description here\n\"\"\"").unwrap();
    assert_eq!(
        schema["properties"]["baz"]["description"],
        "a longer description here"
    );
}

#[test]
fn heredoc_preserves_internal_newlines() {
    let schema = parse_schema_dsl("baz: \"\"\"\nline one\nline two\n\"\"\"").unwrap();
    assert_eq!(
        schema["properties"]["baz"]["description"],
        "line one\nline two"
    );
}

#[test]
fn line_continuation_between_name_and_type() {
    let schema = parse_schema_dsl("?age \\\n      int").unwrap();
    assert_eq!(schema["properties"]["age"]["type"], "integer");
    assert!(schema.get("required").is_none());
}

#[test]
fn line_continuation_in_inline_description() {
    let schema = parse_schema_dsl("name: a long \\\n  description here").unwrap();
    assert_eq!(
        schema["properties"]["name"]["description"],
        "a long description here"
    );
}

#[test]
fn full_complex_example() {
    let input = r#"people {
    name
    ?age int
    misc [any]: whatever you want
    ?nested { data [string] }
}: here is the people description,
foo [string]|int, bar bool: "hello, universe""#;

    let schema = parse_schema_dsl(input).unwrap();

    // Top-level required
    assert_eq!(schema["required"], json!(["people", "foo", "bar"]));

    // people object
    let people = &schema["properties"]["people"];
    assert_eq!(people["description"], "here is the people description");
    assert_eq!(people["required"], json!(["name", "misc"]));

    // people.misc
    assert_eq!(
        people["properties"]["misc"],
        json!({
            "type": "array",
            "items": {},
            "description": "whatever you want"
        })
    );

    // people.nested (optional)
    let nested = &people["properties"]["nested"];
    assert_eq!(
        nested["properties"]["data"],
        json!({
            "type": "array",
            "items": { "type": "string" }
        })
    );

    // foo union
    assert_eq!(
        schema["properties"]["foo"],
        json!({
            "anyOf": [
                { "type": "array", "items": { "type": "string" } },
                { "type": "integer" }
            ]
        })
    );

    // bar
    assert_eq!(
        schema["properties"]["bar"],
        json!({
            "type": "boolean",
            "description": "hello, universe"
        })
    );
}

#[test]
fn newline_separated_in_nested_object() {
    let input = "person {\n    name\n    age int\n}";
    let schema = parse_schema_dsl(input).unwrap();
    let person = &schema["properties"]["person"];
    assert_eq!(person["properties"]["name"]["type"], "string");
    assert_eq!(person["properties"]["age"]["type"], "integer");
}

#[test]
fn comma_before_newline() {
    let input = "name,\nage int";
    let schema = parse_schema_dsl(input).unwrap();
    assert_eq!(schema["required"], json!(["name", "age"]));
}

#[test]
fn string_literal_enum() {
    let schema = parse_schema_dsl(r#"status "active"|"inactive"|"archived""#).unwrap();
    assert_eq!(
        schema["properties"]["status"],
        json!({"enum": ["active", "inactive", "archived"]})
    );
}

#[test]
fn single_string_literal() {
    let schema = parse_schema_dsl(r#"kind "fixed""#).unwrap();
    assert_eq!(schema["properties"]["kind"], json!({"const": "fixed"}));
}

#[test]
fn number_literal() {
    let schema = parse_schema_dsl("version 1").unwrap();
    assert_eq!(schema["properties"]["version"], json!({"const": 1}));
}

#[test]
fn negative_number_literal() {
    let schema = parse_schema_dsl("offset -1").unwrap();
    assert_eq!(schema["properties"]["offset"], json!({"const": -1}));
}

#[test]
fn float_literal() {
    let schema = parse_schema_dsl("ratio 0.5").unwrap();
    assert_eq!(schema["properties"]["ratio"], json!({"const": 0.5}));
}

#[test]
fn mixed_literal_enum() {
    let schema = parse_schema_dsl(r#"value "foo"|"bar"|42"#).unwrap();
    assert_eq!(
        schema["properties"]["value"],
        json!({"enum": ["foo", "bar", 42]})
    );
}

#[test]
fn literal_mixed_with_type() {
    let schema = parse_schema_dsl(r#"value "special"|int"#).unwrap();
    assert_eq!(
        schema["properties"]["value"],
        json!({"anyOf": [{"const": "special"}, {"type": "integer"}]})
    );
}

#[test]
fn boolean_literal_true() {
    let schema = parse_schema_dsl("answer true").unwrap();
    assert_eq!(schema["properties"]["answer"], json!({"const": true}));
}

#[test]
fn boolean_literal_false() {
    let schema = parse_schema_dsl("answer false").unwrap();
    assert_eq!(schema["properties"]["answer"], json!({"const": false}));
}

#[test]
fn null_literal() {
    let schema = parse_schema_dsl("cleared null").unwrap();
    assert_eq!(schema["properties"]["cleared"], json!({"const": null}));
}

#[test]
fn nullable_string() {
    let schema = parse_schema_dsl("value null|string").unwrap();
    assert_eq!(
        schema["properties"]["value"],
        json!({"anyOf": [{"const": null}, {"type": "string"}]})
    );
}

#[test]
fn enum_in_array() {
    let schema = parse_schema_dsl(r#"tags ["foo"|"bar"|"baz"]"#).unwrap();
    assert_eq!(
        schema["properties"]["tags"],
        json!({"type": "array", "items": {"enum": ["foo", "bar", "baz"]}})
    );
}

#[test]
fn enum_in_nested_object() {
    let schema = parse_schema_dsl(r#"config { mode "fast"|"slow", count int }"#).unwrap();
    let config = &schema["properties"]["config"];
    assert_eq!(
        config["properties"]["mode"],
        json!({"enum": ["fast", "slow"]})
    );
    assert_eq!(config["properties"]["count"], json!({"type": "integer"}));
}

#[test]
fn enum_with_description() {
    let schema = parse_schema_dsl(r#"status "active"|"inactive": current status"#).unwrap();
    assert_eq!(
        schema["properties"]["status"],
        json!({"enum": ["active", "inactive"], "description": "current status"})
    );
}

#[test]
fn true_false_enum_is_not_bool_type() {
    // true|false produces enum, not {"type": "boolean"}
    let schema = parse_schema_dsl("flag true|false").unwrap();
    assert_eq!(schema["properties"]["flag"], json!({"enum": [true, false]}));
}

#[test]
fn field_level_union_with_literal_and_array() {
    let schema = parse_schema_dsl(r#"value ["a"|"b"]|int"#).unwrap();
    assert_eq!(
        schema["properties"]["value"],
        json!({
            "anyOf": [
                {"type": "array", "items": {"enum": ["a", "b"]}},
                {"type": "integer"}
            ]
        })
    );
}
