use jp_tool::ToolDocs;
use serde_json::json;

use super::*;

fn args(value: &Value) -> Map<String, Value> {
    value.as_object().expect("the fixture is an object").clone()
}

/// A tool whose own schema is `parameters`.
fn tool(parameters: Value) -> ToolDefinition {
    ToolDefinition {
        name: "fs_read_file".to_owned(),
        docs: ToolDocs {
            summary: Some("Read a file.".to_owned()),
            ..ToolDocs::default()
        },
        parameters,
    }
}

/// A tool taking a required `path`, and nothing called `ops`.
fn reader() -> ToolDefinition {
    tool(json!({
        "type": "object",
        "properties": { "path": { "type": "string" } },
        "required": ["path"],
    }))
}

/// A tool whose own schema declares an `ops` parameter.
fn migrator(required: &[&str]) -> ToolDefinition {
    tool(json!({
        "type": "object",
        "properties": {
            "ops": { "type": "array", "items": { "type": "object" } },
            "dry_run": { "type": "boolean" },
        },
        "required": required,
    }))
}

#[test]
fn advertise_wraps_the_schema_and_asks_for_batching() {
    let definition = reader();

    let advertised = advertise(&definition);

    assert_eq!(advertised.name, "fs_read_file");
    assert_eq!(advertised.parameters, envelope(&definition.parameters));
    assert_eq!(
        advertised.docs.schema_description(),
        Some(
            "Read a file. Batch every operation you already know you need into one call, each as \
             its own element of `ops`, rather than issuing them one at a time."
        )
    );
}

/// A reference inside the envelope still resolves once a provider inlines it,
/// which is what Ollama needs: it drops `$ref` while decoding a schema.
#[test]
fn inlining_the_envelope_resolves_hoisted_references() {
    let operation = json!({
        "type": "object",
        "properties": {
            "kinds": { "type": "array", "items": { "$ref": "#/$defs/EntryType" } }
        },
        "required": ["kinds"],
        "$defs": { "EntryType": { "type": "string", "enum": ["Enum", "Method"] } }
    });

    let inlined = jp_tool::schema::inline(&envelope(&operation));

    assert_eq!(
        inlined["properties"]["ops"]["items"]["properties"]["kinds"]["items"],
        json!({ "type": "string", "enum": ["Enum", "Method"] })
    );
}

#[test]
fn split_takes_an_envelope_apart() {
    let arguments = args(&json!({ "ops": [{ "path": "a.rs" }, { "path": "b.rs" }] }));

    assert_eq!(
        split(&reader(), &arguments),
        Split::Envelope(vec![
            args(&json!({ "path": "a.rs" })),
            args(&json!({ "path": "b.rs" })),
        ])
    );
}

/// A bare call to a fan-out tool is an ordinary call: the tool's own examples
/// are valid calls as written.
#[test]
fn split_runs_a_call_without_an_envelope_bare() {
    let arguments = args(&json!({ "path": "a.rs" }));

    assert_eq!(split(&reader(), &arguments), Split::Bare);
}

/// For a tool that has no `ops` of its own, an `ops` key can only be the
/// envelope, so a broken one is reported rather than run as a bare call that
/// would fail with a less useful message.
#[test]
fn split_reports_a_malformed_envelope_for_a_tool_without_its_own_key() {
    let arguments = args(&json!({ "ops": [{ "path": "a.rs" }, "b.rs"] }));

    assert_eq!(
        split(&reader(), &arguments),
        Split::Malformed(ExpandError::ElementNotAnObject { index: 1 })
    );
}

/// Operations are validated when the call runs, not when it is split, so one
/// bad operation fails alone rather than refusing the whole call.
#[test]
fn split_leaves_validating_operations_to_the_call_for_a_tool_without_its_own_key() {
    let arguments = args(&json!({ "ops": [{ "pth": "a.rs" }] }));

    assert_eq!(
        split(&reader(), &arguments),
        Split::Envelope(vec![args(&json!({ "pth": "a.rs" }))])
    );
}

/// A tool that declares its own `ops` keeps it whenever the elements are not
/// valid operations of the tool.
#[test]
fn split_leaves_a_tools_own_key_alone_when_its_elements_are_not_operations() {
    let arguments = args(&json!({ "ops": [{ "add_column": "email" }] }));

    assert_eq!(split(&migrator(&[]), &arguments), Split::Bare);
}

/// A shape that is not an envelope at all is the tool's own parameter too.
#[test]
fn split_leaves_a_tools_own_key_alone_when_it_is_not_an_array_of_objects() {
    let arguments = args(&json!({ "ops": [] }));

    assert_eq!(split(&migrator(&[]), &arguments), Split::Bare);
}

/// Both readings validate when the tool requires nothing: the envelope wins,
/// because it is the shape every caller was shown.
#[test]
fn split_prefers_the_envelope_when_both_readings_validate() {
    let arguments = args(&json!({ "ops": [{ "dry_run": true }] }));

    assert_eq!(
        split(&migrator(&[]), &arguments),
        Split::Envelope(vec![args(&json!({ "dry_run": true }))])
    );
}

/// An element missing a parameter the tool requires is not an operation.
#[test]
fn split_checks_required_parameters_for_a_tool_with_its_own_key() {
    let arguments = args(&json!({ "ops": [{ "dry_run": true }] }));

    assert_eq!(split(&migrator(&["ops"]), &arguments), Split::Bare);
}

#[test]
fn envelope_wraps_the_operation_schema_in_a_required_array() {
    let operation = json!({
        "type": "object",
        "properties": { "path": { "type": "string" } },
        "required": ["path"],
    });

    let wrapped = envelope(&operation);

    assert_eq!(
        wrapped,
        json!({
            "type": "object",
            "properties": {
                "ops": {
                    "type": "array",
                    "minItems": 1,
                    "description": "The operations to perform. Each element is one complete set \
                                    of this tool's arguments.",
                    "items": {
                        "type": "object",
                        "properties": { "path": { "type": "string" } },
                        "required": ["path"],
                    },
                }
            },
            "required": ["ops"],
            "additionalProperties": false,
        })
    );
}

/// A same-document reference is anchored at the document root, so the `$defs`
/// block it points at has to move to the envelope's root when the operation
/// schema is nested under it.
#[test]
fn envelope_hoists_definitions_so_references_still_resolve() {
    let operation = json!({
        "type": "object",
        "properties": {
            "kinds": { "type": "array", "items": { "$ref": "#/$defs/EntryType" } }
        },
        "required": ["kinds"],
        "$defs": { "EntryType": { "type": "string", "enum": ["Enum", "Method"] } },
    });

    let wrapped = envelope(&operation);

    assert_eq!(
        wrapped["$defs"],
        json!({ "EntryType": { "type": "string", "enum": ["Enum", "Method"] } }),
        "the definitions block sits at the root the references name"
    );
    assert_eq!(
        wrapped["properties"]["ops"]["items"]["$defs"],
        Value::Null,
        "and is gone from the nested copy, so it is defined exactly once"
    );
    assert_eq!(
        wrapped["properties"]["ops"]["items"]["properties"]["kinds"]["items"],
        json!({ "$ref": "#/$defs/EntryType" }),
        "the reference itself is untouched"
    );
}

/// The older `definitions` spelling moves too.
#[test]
fn envelope_hoists_the_legacy_definitions_spelling() {
    let operation = json!({
        "type": "object",
        "properties": { "kind": { "$ref": "#/definitions/Kind" } },
        "definitions": { "Kind": { "type": "string" } },
    });

    let wrapped = envelope(&operation);

    assert_eq!(
        wrapped["definitions"],
        json!({ "Kind": { "type": "string" } })
    );
    assert_eq!(
        wrapped["properties"]["ops"]["items"]["definitions"],
        Value::Null
    );
}

#[test]
fn expand_returns_one_argument_map_per_operation() {
    let arguments = args(&json!({
        "ops": [
            { "path": "a.rs", "start_line": 1 },
            { "path": "b.rs" },
        ]
    }));

    let ops = expand(&arguments).expect("the envelope is well formed");

    assert_eq!(ops, vec![
        args(&json!({ "path": "a.rs", "start_line": 1 })),
        args(&json!({ "path": "b.rs" })),
    ]);
}

#[test]
fn expand_accepts_a_single_operation() {
    let arguments = args(&json!({ "ops": [{ "path": "a.rs" }] }));

    let ops = expand(&arguments).expect("one operation is a valid call");

    assert_eq!(ops, vec![args(&json!({ "path": "a.rs" }))]);
}

#[test]
fn expand_reports_a_missing_envelope() {
    let arguments = args(&json!({ "path": "a.rs" }));

    assert_eq!(expand(&arguments), Err(ExpandError::Missing));
}

#[test]
fn expand_rejects_an_empty_array() {
    let arguments = args(&json!({ "ops": [] }));

    assert_eq!(expand(&arguments), Err(ExpandError::Empty));
}

#[test]
fn expand_rejects_a_non_array_envelope() {
    let arguments = args(&json!({ "ops": { "path": "a.rs" } }));

    assert_eq!(expand(&arguments), Err(ExpandError::NotAnArray));
}

#[test]
fn expand_names_the_position_of_a_non_object_element() {
    let arguments = args(&json!({ "ops": [{ "path": "a.rs" }, "b.rs"] }));

    assert_eq!(
        expand(&arguments),
        Err(ExpandError::ElementNotAnObject { index: 1 })
    );
}

#[test]
fn expand_error_messages_name_the_tool_and_the_envelope() {
    assert_eq!(
        ExpandError::NotAnArray.message("fs_read_file"),
        "Tool 'fs_read_file' expects `ops` to be an array of operations, and the call gave it \
         something else."
    );
    assert_eq!(
        ExpandError::Empty.message("fs_read_file"),
        "Tool 'fs_read_file' was called with an empty `ops` array, so there was nothing to do. \
         Include at least one operation."
    );
    assert_eq!(
        ExpandError::ElementNotAnObject { index: 2 }.message("fs_read_file"),
        "Tool 'fs_read_file' expects every element of `ops` to be an object holding one \
         operation's arguments; element 2 was not."
    );
}

/// One operation reads exactly like a call to the same tool without fan-out,
/// success or failure, which is what keeps the envelope invisible at N=1.
#[test]
fn fold_returns_a_lone_success_without_any_framing() {
    let folded = fold(&[OperationOutcome::Ok("file contents".to_owned())]);

    assert_eq!(folded, ToolResult::text("file contents"));
}

#[test]
fn fold_returns_a_lone_failure_as_an_unframed_error() {
    let folded = fold(&[OperationOutcome::Error("not found".to_owned())]);

    assert_eq!(folded, ToolResult::error("not found"));
}

/// One success is enough for the call to have done something, so it is not an
/// error; the failed sections say which operations did not.
#[test]
fn fold_frames_each_operation_with_its_position() {
    let folded = fold(&[
        OperationOutcome::Ok("first".to_owned()),
        OperationOutcome::Error("second failed".to_owned()),
        OperationOutcome::Ok("third".to_owned()),
    ]);

    assert_eq!(
        folded,
        ToolResult::text("[1/3] ok\nfirst\n\n[2/3] error\nsecond failed\n\n[3/3] ok\nthird\n")
    );
}

/// A call none of whose operations succeeded is an error, so the caller sees it
/// flagged the way it would see a bare call to the same tool fail.
#[test]
fn fold_reports_a_call_with_no_success_as_an_error() {
    let folded = fold(&[
        OperationOutcome::Error("first failed".to_owned()),
        OperationOutcome::NotRun { after: 1 },
    ]);

    assert_eq!(
        folded,
        ToolResult::error(
            "[1/2] error\nfirst failed\n\n[2/2] not run (stopped after operation 1 failed)\n"
        )
    );
}

/// Without these lines a model that asked for five operations and reads three
/// assumes the other two succeeded silently.
#[test]
fn fold_names_the_operations_that_never_started() {
    let folded = fold(&[
        OperationOutcome::Ok("File deleted.".to_owned()),
        OperationOutcome::Error("File has uncommitted changes.".to_owned()),
        OperationOutcome::NotRun { after: 2 },
        OperationOutcome::NotRun { after: 2 },
    ]);

    assert_eq!(
        folded,
        ToolResult::text(
            "[1/4] ok\nFile deleted.\n\n[2/4] error\nFile has uncommitted changes.\n\n[3/4] not \
             run (stopped after operation 2 failed)\n\n[4/4] not run (stopped after operation 2 \
             failed)\n"
        )
    );
}
