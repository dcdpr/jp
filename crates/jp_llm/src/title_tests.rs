use super::*;

#[test]
fn title_schema_has_correct_structure() {
    let schema = title_schema(3);

    assert_eq!(schema["type"], "object");
    assert_eq!(schema["properties"]["titles"]["type"], "array");
    assert_eq!(schema["properties"]["titles"]["minItems"], 3);
    assert_eq!(schema["properties"]["titles"]["maxItems"], 3);
    assert!(
        schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!("titles"))
    );
    assert_eq!(schema["additionalProperties"], false);
}

#[test]
fn title_schema_single_title() {
    let schema = title_schema(1);
    assert_eq!(schema["properties"]["titles"]["minItems"], 1);
    assert_eq!(schema["properties"]["titles"]["maxItems"], 1);
}

#[test]
fn title_instructions_without_rejected() {
    let sections = title_instructions(3, &[]);
    assert_eq!(sections.len(), 1);
}

#[test]
fn title_instructions_with_rejected() {
    let rejected = vec!["Bad Title".to_owned(), "Worse Title".to_owned()];
    let sections = title_instructions(3, &rejected);
    assert_eq!(sections.len(), 2);
}

#[test]
fn extract_titles_valid() {
    let data = json!({"titles": ["Title A", "Title B"]});
    assert_eq!(extract_titles(&data), vec!["Title A", "Title B"]);
}

#[test]
fn extract_titles_missing_key() {
    let data = json!({"other": "value"});
    assert!(extract_titles(&data).is_empty());
}

#[test]
fn extract_titles_wrong_type() {
    let data = json!({"titles": "not an array"});
    assert!(extract_titles(&data).is_empty());
}

#[test]
fn extract_titles_mixed_types_filters_non_strings() {
    let data = json!({"titles": ["Valid", 42, null, "Also Valid"]});
    assert_eq!(extract_titles(&data), vec!["Valid", "Also Valid"]);
}
