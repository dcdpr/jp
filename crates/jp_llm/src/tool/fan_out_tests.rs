use jp_config::conversation::tool::FanOutOnError;
use serde_json::json;

use super::*;

fn args(value: &Value) -> Map<String, Value> {
    value.as_object().expect("the fixture is an object").clone()
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

/// A tool whose schema says `ops` and which receives `path` was called wrongly.
/// Running it anyway would hide the mistake from the model that made it, and
/// teach it that the envelope is optional.
#[test]
fn expand_rejects_a_call_that_skipped_the_envelope() {
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
        ExpandError::Missing.message("fs_read_file"),
        "Tool 'fs_read_file' takes its arguments in an `ops` array, but the call had no `ops` \
         key. Wrap the arguments in one: {\"ops\": [{...}]}."
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

/// One successful operation reads exactly like a call to the same tool without
/// fan-out, which is what keeps the envelope invisible at N=1.
#[test]
fn fold_returns_a_lone_success_without_any_framing() {
    let folded = fold(&[OperationOutcome::Ok("file contents".to_owned())]);

    assert_eq!(folded, "file contents");
}

/// A lone *failure* still gets framing: the assistant needs to see that the one
/// operation it asked for is the one that failed.
#[test]
fn fold_frames_a_lone_failure() {
    let folded = fold(&[OperationOutcome::Error("not found".to_owned())]);

    assert_eq!(folded, "[1/1] error\nnot found\n");
}

#[test]
fn fold_frames_each_operation_with_its_position() {
    let folded = fold(&[
        OperationOutcome::Ok("first".to_owned()),
        OperationOutcome::Error("second failed".to_owned()),
        OperationOutcome::Ok("third".to_owned()),
    ]);

    assert_eq!(
        folded,
        "[1/3] ok\nfirst\n\n[2/3] error\nsecond failed\n\n[3/3] ok\nthird\n"
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
        "[1/4] ok\nFile deleted.\n\n[2/4] error\nFile has uncommitted changes.\n\n[3/4] not run \
         (stopped after operation 2 failed)\n\n[4/4] not run (stopped after operation 2 failed)\n"
    );
}

#[test]
fn should_stop_is_false_while_nothing_has_failed() {
    let fan_out = FanOut {
        concurrency: Some(1),
        on_error: FanOutOnError::Stop,
    };

    assert!(!should_stop(fan_out, &[OperationOutcome::Ok(
        "ok".to_owned()
    )]));
}

#[test]
fn should_stop_is_true_after_a_failure_under_stop() {
    let fan_out = FanOut {
        concurrency: Some(1),
        on_error: FanOutOnError::Stop,
    };

    assert!(should_stop(fan_out, &[
        OperationOutcome::Ok("ok".to_owned()),
        OperationOutcome::Error("boom".to_owned()),
    ]));
}

#[test]
fn should_stop_stays_false_under_continue() {
    let fan_out = FanOut {
        concurrency: Some(1),
        on_error: FanOutOnError::Continue,
    };

    assert!(!should_stop(fan_out, &[OperationOutcome::Error(
        "boom".to_owned()
    )]));
}
