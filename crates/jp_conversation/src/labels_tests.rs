use super::*;

/// The values held under `key`, for readable assertions.
fn values<'a>(labels: &'a Labels, key: &str) -> Vec<&'a str> {
    labels
        .get(key)
        .map(|values| values.iter().map(String::as_str).collect())
        .unwrap_or_default()
}

#[test]
fn insert_accumulates_in_the_order_values_were_added() {
    let mut labels = Labels::default();

    assert!(labels.insert("crate", "jp_config"));
    assert!(labels.insert("crate", "jp_llm"));

    assert_eq!(values(&labels, "crate"), ["jp_config", "jp_llm"]);
}

#[test]
fn inserting_a_value_the_key_already_holds_changes_nothing() {
    let mut labels = Labels::from_iter([("crate", ["jp_config"])]);

    assert!(!labels.insert("crate", "jp_config"));
    assert_eq!(labels, Labels::from_iter([("crate", ["jp_config"])]));
}

#[test]
fn set_replaces_the_key_and_returns_what_it_displaced() {
    let mut labels = Labels::from_iter([("crate", vec!["jp_config", "jp_llm"])]);

    let displaced = labels.set("crate", ["jp_cli"]);

    assert_eq!(displaced.iter().collect::<Vec<_>>(), vec![
        "jp_config",
        "jp_llm"
    ]);
    assert_eq!(labels, Labels::from_iter([("crate", ["jp_cli"])]));
}

/// A key that was absent displaces nothing, rather than reporting an empty set
/// that a caller could mistake for a key it emptied.
#[test]
fn set_on_an_absent_key_displaces_nothing() {
    let mut labels = Labels::default();

    assert!(labels.set("crate", ["jp_cli"]).is_empty());
}

#[test]
fn set_with_no_values_removes_the_key() {
    let mut labels = Labels::from_iter([("crate", ["jp_config"])]);

    let displaced = labels.set("crate", Vec::<String>::new());

    assert_eq!(displaced.iter().collect::<Vec<_>>(), vec!["jp_config"]);
    assert!(labels.is_empty());
}

#[test]
fn set_deduplicates_its_input() {
    let mut labels = Labels::default();

    labels.set("crate", ["jp_llm", "jp_config", "jp_llm"]);

    assert_eq!(
        values(&labels, "crate"),
        ["jp_llm", "jp_config"],
        "the first occurrence wins"
    );
}

#[test]
fn remove_key_returns_the_values_it_held() {
    let mut labels = Labels::from_iter([("crate", vec!["jp_config", "jp_llm"])]);

    let removed = labels.remove_key("crate").unwrap();

    assert_eq!(removed.iter().collect::<Vec<_>>(), vec![
        "jp_config",
        "jp_llm"
    ]);
    assert!(labels.remove_key("crate").is_none());
}

#[test]
fn remove_value_drops_the_key_when_it_empties() {
    let mut labels = Labels::from_iter([("crate", ["jp_config"])]);

    assert!(labels.remove_value("crate", "jp_config"));
    assert!(labels.is_empty(), "the key went with its last value");
}

#[test]
fn remove_value_keeps_the_order_of_what_remains() {
    let mut labels = Labels::from_iter([("crate", vec!["jp_config", "jp_llm", "jp_cli"])]);

    assert!(labels.remove_value("crate", "jp_llm"));

    assert_eq!(values(&labels, "crate"), ["jp_config", "jp_cli"]);
}

#[test]
fn removing_a_value_a_key_does_not_hold_reports_it() {
    let mut labels = Labels::from_iter([("crate", ["jp_config"])]);

    assert!(!labels.remove_value("crate", "jp_llm"));
    assert!(!labels.remove_value("absent", "jp_config"));
    assert_eq!(labels, Labels::from_iter([("crate", ["jp_config"])]));
}

#[test]
fn contains_matches_on_set_membership() {
    let labels = Labels::from_iter([("crate", ["jp_config", "jp_llm"])]);

    assert!(labels.contains("crate", "jp_config"));
    assert!(labels.contains("crate", "jp_llm"));
    assert!(!labels.contains("crate", "jp_cli"));
    assert!(!labels.contains("absent", "jp_config"));
}

// ── On-disk shape ────────────────────────────────────────────────────────────

#[test]
fn a_scalar_reads_as_a_one_element_set() {
    let labels: Labels = serde_json::from_str(r#"{"branch":"main","draft":""}"#).unwrap();

    assert_eq!(
        labels,
        Labels::from_iter([("branch", ["main"]), ("draft", [""])])
    );
}

#[test]
fn an_array_is_deduplicated_with_the_first_occurrence_winning() {
    let labels: Labels =
        serde_json::from_str(r#"{"crate":["jp_llm","jp_config","jp_llm"]}"#).unwrap();

    assert_eq!(values(&labels, "crate"), ["jp_llm", "jp_config"]);
}

/// A hand-edited file can hold `{"crate": []}`, which the API cannot produce.
/// Reading normalizes it away rather than failing the whole conversation.
#[test]
fn an_empty_array_drops_the_key() {
    let labels: Labels = serde_json::from_str(r#"{"crate":[],"branch":["main"]}"#).unwrap();

    assert_eq!(labels, Labels::from_iter([("branch", ["main"])]));
}

#[test]
fn a_non_string_value_is_an_error() {
    let error = serde_json::from_str::<Labels>(r#"{"crate":42}"#).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("expected a string or an array of strings"),
        "got: {error}"
    );
}

#[test]
fn a_non_string_inside_an_array_is_an_error() {
    let error = serde_json::from_str::<Labels>(r#"{"crate":["jp_config",42]}"#).unwrap_err();

    assert!(error.to_string().contains("invalid type"), "got: {error}");
}

#[test]
fn values_are_always_written_as_an_array() {
    let labels = Labels::from_iter([("crate", vec!["jp_config", "jp_llm"]), ("draft", vec![""])]);

    assert_eq!(
        serde_json::to_string(&labels).unwrap(),
        r#"{"crate":["jp_config","jp_llm"],"draft":[""]}"#
    );
}

/// Keys are sorted on the way out, whatever order they were added in, so
/// committed metadata does not churn.
#[test]
fn keys_are_written_sorted() {
    let mut labels = Labels::default();
    labels.insert("team", "platform");
    labels.insert("branch", "main");

    assert_eq!(
        serde_json::to_string(&labels).unwrap(),
        r#"{"branch":["main"],"team":["platform"]}"#
    );
}

/// A conversation written before this shape existed carries scalars; it loads
/// unchanged and is rewritten as arrays.
#[test]
fn a_scalar_file_round_trips_into_the_array_form() {
    let labels: Labels = serde_json::from_str(r#"{"branch":"main"}"#).unwrap();

    assert_eq!(
        serde_json::to_string(&labels).unwrap(),
        r#"{"branch":["main"]}"#
    );
}
