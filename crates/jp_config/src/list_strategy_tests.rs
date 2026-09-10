use schematic::{
    Config, ConfigError, PartialConfig as _, SchemaBuilder, SchemaType, Schematic as _,
};
use serde_json::{from_str as from_json, to_string as to_json};
use toml::{from_str as from_toml, to_string as to_toml};

use crate::{
    PartialAppConfig, assignment::AssignKeyValue as _, editor::EditorConfig,
    providers::mcp::PartialStdioConfig, types::vec::MergeableVec,
};

/// Scalar-list wrappers exercise the declared field's outer containers.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
#[expect(
    clippy::box_collection,
    reason = "the fixture tests boxed macro fields"
)]
struct WrappedLists {
    #[setting(partial_via = MergeableVec::<String>)]
    optional: Option<Vec<String>>,
    #[setting(partial_via = MergeableVec::<String>)]
    boxed: Box<Vec<String>>,
    #[setting(partial_via = MergeableVec::<String>)]
    optional_boxed: Option<Box<Vec<String>>>,
    #[setting(required, partial_via = MergeableVec::<String>)]
    required_boxed: Box<Vec<String>>,
}

#[test]
fn scalar_list_via_preserves_optional_and_boxed_fields() {
    let partial = from_json(
        r#"{
        "optional": {"value":["optional"],"strategy":"replace"},
        "boxed": ["boxed"],
        "optional_boxed": ["optional_boxed"],
        "required_boxed": ["required_boxed"]
    }"#,
    )
    .unwrap();
    let config = WrappedLists::from_partial(partial, vec![]).unwrap();
    assert_eq!(config.optional, Some(vec!["optional".to_owned()]));
    assert_eq!(*config.boxed, vec!["boxed".to_owned()]);
    assert_eq!(
        config.optional_boxed.as_deref(),
        Some(&vec!["optional_boxed".to_owned()])
    );
    assert_eq!(*config.required_boxed, vec!["required_boxed".to_owned()]);
}

#[test]
fn scalar_list_via_preserves_absence_defaults_and_required_validation() {
    let partial = PartialWrappedLists {
        required_boxed: Some(vec!["required".to_owned()].into()),
        ..Default::default()
    };
    let config = WrappedLists::from_partial(partial, vec![]).unwrap();
    assert_eq!(config.optional, None);
    assert_eq!(*config.boxed, Vec::<String>::new());
    assert_eq!(config.optional_boxed, None);

    let error = WrappedLists::from_partial(PartialWrappedLists::empty(), vec![]).unwrap_err();
    let ConfigError::MissingRequired { fields } = error else {
        panic!("expected a missing required field, got {error}");
    };
    assert_eq!(fields, ["required_boxed"]);
}

#[test]
fn object_merge_assignment_appends_and_preserves_repeated_flags() {
    let mut partial = PartialStdioConfig {
        arguments: Some(vec!["serve".to_owned(), "--flag".to_owned(), "x".to_owned()].into()),
        ..Default::default()
    };
    partial
        .assign(
            r#"arguments:+={"value":["--flag","y"],"strategy":"append"}"#
                .parse()
                .unwrap(),
        )
        .unwrap();

    assert_eq!(
        partial.arguments.as_deref(),
        Some(&vec![
            "serve".to_owned(),
            "--flag".to_owned(),
            "x".to_owned(),
            "--flag".to_owned(),
            "y".to_owned(),
        ])
    );
}

#[test]
fn object_merge_assignment_prepends() {
    let mut partial = PartialStdioConfig {
        arguments: Some(vec!["serve".to_owned()].into()),
        ..Default::default()
    };
    partial
        .assign(
            r#"arguments:+={"value":["--verbose"],"strategy":"prepend"}"#
                .parse()
                .unwrap(),
        )
        .unwrap();

    assert_eq!(
        partial.arguments.as_deref(),
        Some(&vec!["--verbose".to_owned(), "serve".to_owned()])
    );
}

#[test]
fn object_assignment_and_replace_strategy_replace() {
    let mut partial = PartialStdioConfig {
        arguments: Some(vec!["serve".to_owned()].into()),
        ..Default::default()
    };
    partial
        .assign(
            r#"arguments:={"value":["first"],"strategy":"append"}"#
                .parse()
                .unwrap(),
        )
        .unwrap();
    assert_eq!(
        partial.arguments.as_deref(),
        Some(&vec!["first".to_owned()])
    );

    partial
        .assign(
            r#"arguments:+={"value":["second"],"strategy":"replace"}"#
                .parse()
                .unwrap(),
        )
        .unwrap();
    assert_eq!(
        partial.arguments.as_deref(),
        Some(&vec!["second".to_owned()])
    );
}

#[test]
fn object_merge_assignment_uses_the_fields_dedup_policy() {
    let mut partial = PartialAppConfig::empty();
    partial
        .assign("editor.envs=EDITOR,VISUAL".parse().unwrap())
        .unwrap();
    partial
        .assign(
            r#"editor.envs:+={"value":["VISUAL","MY_EDITOR"],"strategy":"append"}"#
                .parse()
                .unwrap(),
        )
        .unwrap();

    assert_eq!(
        partial.editor.envs.as_deref(),
        Some(&vec![
            "EDITOR".to_owned(),
            "VISUAL".to_owned(),
            "MY_EDITOR".to_owned(),
        ])
    );
}

#[test]
fn appending_to_a_replacement_survives_serialization_and_layering() {
    let mut overlay = PartialAppConfig::empty();
    overlay
        .assign(
            r#"editor.envs:={"value":["VISUAL"],"strategy":"replace"}"#
                .parse()
                .unwrap(),
        )
        .unwrap();
    overlay
        .assign("editor.envs+=MY_EDITOR".parse().unwrap())
        .unwrap();

    let serialized = to_json(&overlay.editor.envs).unwrap();
    // MergedVec serializes its discard flag even when it is false.
    assert_eq!(
        serialized,
        r#"{"value":["VISUAL","MY_EDITOR"],"strategy":"replace","discard_when_merged":false}"#
    );
    let overlay = from_toml(&to_toml(&overlay).unwrap()).unwrap();
    let mut base = PartialAppConfig::empty();
    base.assign("editor.envs=EDITOR".parse().unwrap()).unwrap();
    base.merge(&(), overlay).unwrap();

    assert_eq!(
        base.editor.envs.as_deref(),
        Some(&vec!["VISUAL".to_owned(), "MY_EDITOR".to_owned()])
    );
}

#[test]
fn indexed_assignment_preserves_metadata_but_whole_list_assignment_replaces_it() {
    let mut partial = PartialAppConfig::empty();
    partial.assign(r#"editor.envs:={"value":["VISUAL"],"strategy":"replace","dedup":false,"discard_when_merged":true}"#.parse().unwrap()).unwrap();
    partial
        .assign("editor.envs.0=MY_EDITOR".parse().unwrap())
        .unwrap();
    assert_eq!(
        to_json(&partial.editor.envs).unwrap(),
        r#"{"value":["MY_EDITOR"],"strategy":"replace","dedup":false,"discard_when_merged":true}"#
    );

    partial
        .assign("editor.envs=EDITOR".parse().unwrap())
        .unwrap();
    assert_eq!(to_json(&partial.editor.envs).unwrap(), r#"["EDITOR"]"#);
    partial
        .assign("editor.envs:=null".parse().unwrap())
        .unwrap();
    assert_eq!(partial.editor.envs, None);
}

#[test]
fn scalar_list_schema_describes_array_and_strategy_object() {
    let SchemaType::Struct(editor) = EditorConfig::build_schema(SchemaBuilder::default()).ty else {
        panic!("editor schema must be a struct");
    };
    let SchemaType::Union(envs) = &editor.fields["envs"].schema.ty else {
        panic!("envs schema must accept both a list and a strategy object");
    };
    assert_eq!(envs.variants_types.len(), 2);
    assert!(matches!(envs.variants_types[0].ty, SchemaType::Array(_)));
    let SchemaType::Struct(object) = &envs.variants_types[1].ty else {
        panic!("the expanded list schema must be an object");
    };
    assert!(matches!(
        object.fields["value"].schema.ty,
        SchemaType::Array(_)
    ));
    assert!(object.fields.contains_key("strategy"));
    assert!(object.fields.contains_key("dedup"));
    assert!(object.fields.contains_key("discard_when_merged"));
}
