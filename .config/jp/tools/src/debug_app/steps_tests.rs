use std::path::{Path, PathBuf};

use serde_json::json;

use super::{DRIVER_VERBS, SNAPSHOT, VOCABULARY, parse};

#[test]
fn reads_a_list_of_steps() {
    let steps = parse(&json!([
        {"select": {"identifier": "sidebar.row.jp-c12345"}},
        {"wait_for": {"identifier": "transcript.scroll", "timeout_ms": 5000}},
        {"snapshot": {}}
    ]))
    .unwrap();

    assert_eq!(steps.len(), 3);
    assert_eq!(
        steps[0].json(),
        r#"{"select":{"identifier":"sidebar.row.jp-c12345"}}"#
    );
    assert_eq!(
        steps[0].label(),
        r#"select {"identifier":"sidebar.row.jp-c12345"}"#
    );
    assert!(!steps[0].is_snapshot());
    assert!(steps[2].is_snapshot());
}

/// A step with nothing to address reads as its verb alone, rather than as a
/// verb followed by an empty object.
#[test]
fn labels_an_empty_payload_as_the_verb_alone() {
    let steps = parse(&json!([{"snapshot": {}}])).unwrap();

    assert_eq!(steps[0].label(), "snapshot");
    assert_eq!(steps[0].json(), r#"{"snapshot":{}}"#);
}

/// The driver reads the payload, so an unrecognised key inside one is its error
/// to report, not this parser's.
#[test]
fn passes_an_unrecognised_payload_through_to_the_driver() {
    let steps = parse(&json!([{"click": {"identifier": "a", "unknown": 1}}])).unwrap();

    assert_eq!(
        steps[0].json(),
        r#"{"click":{"identifier":"a","unknown":1}}"#
    );
}

/// A list argument sometimes arrives as a JSON string holding the array, which
/// is the list it represents rather than a step named after raw JSON text.
#[test]
fn reads_a_list_that_arrived_as_a_string() {
    let steps = parse(&json!(r#"[{"press": {"identifier": "sidebar.filter"}}]"#)).unwrap();

    assert_eq!(steps.len(), 1);
    assert_eq!(
        steps[0].json(),
        r#"{"press":{"identifier":"sidebar.filter"}}"#
    );
}

#[test]
fn rejects_a_list_that_is_not_an_array() {
    let error = parse(&json!({"select": {"identifier": "a"}}))
        .unwrap_err()
        .to_string();

    assert!(
        error.starts_with("`steps` is a JSON array of steps, but a object was given."),
        "{error}"
    );
}

#[test]
fn rejects_an_empty_list() {
    let error = parse(&json!([])).unwrap_err().to_string();

    assert!(
        error.starts_with("`steps` is empty, so there is nothing to do."),
        "{error}"
    );
}

#[test]
fn rejects_a_step_that_is_not_an_object() {
    let error = parse(&json!([{"snapshot": {}}, "click"]))
        .unwrap_err()
        .to_string();

    assert!(
        error.starts_with("Step 2 is a string, not an object."),
        "{error}"
    );
}

/// Two verbs in one object have no order the driver could run them in, and
/// guessing one would run half of what was written.
#[test]
fn rejects_a_step_naming_two_verbs() {
    let error = parse(&json!([{"click": {"identifier": "a"}, "press": {"identifier": "b"}}]))
        .unwrap_err()
        .to_string();

    assert!(
        error.starts_with("Step 1 names 2 verbs (click, press)."),
        "{error}"
    );
}

#[test]
fn rejects_an_unknown_verb_and_lists_the_known_ones() {
    let error = parse(&json!([{"scroll": {"identifier": "a"}}]))
        .unwrap_err()
        .to_string();

    assert_eq!(
        error,
        "Step 1 names an unknown verb `scroll`. A step is a single-key object naming one of: \
         click, drag, menu, perform, press, resize, select, snapshot, type, wait_for. For example \
         `{\"select\": {\"identifier\": \"sidebar.row.jp-c12345\"}}`."
    );
}

/// The vocabulary has no fixed-duration wait on purpose, so asking for one gets
/// pointed at the predicate that replaces it rather than at the list of verbs.
#[test]
fn rejects_a_sleep_by_naming_what_to_use_instead() {
    for verb in ["sleep", "delay", "pause", "wait"] {
        let error = parse(&json!([{verb: {"ms": 500}}]))
            .unwrap_err()
            .to_string();

        assert_eq!(
            error,
            format!(
                "Step 1 names `{verb}`, and there is no step that waits a fixed duration: waiting \
                 and assuming the work finished is a guess. Use `wait_for` against an identifier \
                 the app publishes once the work is done. If there is no such identifier, the app \
                 is missing one, and adding it is the fix."
            )
        );
    }
}

/// Every error quotes the vocabulary so a caller can correct in place, which
/// only helps if it names the verbs that actually exist.
#[test]
fn the_quoted_vocabulary_names_every_verb() {
    for verb in DRIVER_VERBS.iter().chain([&SNAPSHOT]) {
        assert!(
            VOCABULARY.contains(verb),
            "the vocabulary quoted in errors omits `{verb}`: {VOCABULARY}"
        );
    }
}

/// The verbs a tool definition advertises, taken from the bullets in its
/// `steps` description.
fn advertised(tool: &str) -> Vec<String> {
    let path = manifest(tool);
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    let value: toml::Value = toml::from_str(&content).expect("valid TOML");

    let description = value["conversation"]["tools"][tool]["parameters"]["steps"]["description"]
        .as_str()
        .expect("`steps` has a description");

    let mut verbs: Vec<String> = description
        .lines()
        .filter_map(|line| line.strip_prefix("- `{\""))
        .filter_map(|rest| rest.split('"').next())
        .map(str::to_owned)
        .collect();
    verbs.sort();
    verbs
}

fn manifest(tool: &str) -> PathBuf {
    let name = tool.trim_start_matches("debug_app_");
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../../.jp/mcp/tools/debug_app/{name}.toml"))
}

/// The definition is what the model writes a list against, and this parser is
/// what accepts it.
/// A verb advertised but unknown here is a step list refused for naming exactly
/// what it was told to name.
#[test]
fn the_tool_definition_advertises_the_verbs_that_exist() {
    let mut every = DRIVER_VERBS.map(str::to_owned).to_vec();
    every.push(SNAPSHOT.to_owned());
    every.sort();

    pretty_assertions::assert_eq!(
        advertised("debug_app_drive"),
        every,
        "the drive definition is out of sync with the vocabulary"
    );
}

/// The whole list is checked before anything runs, because a list abandoned
/// halfway leaves the app in a state nobody asked for.
#[test]
fn rejects_the_list_for_a_bad_step_at_the_end() {
    let error = parse(&json!([
        {"select": {"identifier": "a"}},
        {"snapshot": {}},
        {"teleport": {}}
    ]))
    .unwrap_err()
    .to_string();

    assert!(
        error.starts_with("Step 3 names an unknown verb `teleport`."),
        "{error}"
    );
}
