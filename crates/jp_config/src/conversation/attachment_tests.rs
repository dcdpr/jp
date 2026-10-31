use assert_matches::assert_matches;
use serde_json::json;

use super::*;
use crate::{
    PartialAppConfig,
    assignment::{KvAssignmentError, KvAssignmentErrorKind},
    util::build,
};

/// The attachments a partial lists, in order.
fn attachments(partial: &PartialAppConfig) -> Vec<PartialAttachmentConfig> {
    partial.conversation.attachments.to_vec()
}

/// An attachment written as an object of type `file`.
fn file(path: &str, params: Option<IndexMap<String, JsonValue>>) -> PartialAttachmentConfig {
    PartialAttachmentConfig::Object(PartialAttachmentObjectConfig {
        kind: Some("file".to_owned()),
        path: Some(path.to_owned()),
        params,
    })
}

#[test]
fn an_attachment_object_in_a_json_list_is_assigned_whole() {
    let mut partial = PartialAppConfig::empty();

    partial
        .assign(
            KvAssignment::try_from_cli(
                "conversation.attachments:",
                r#"[{"type": "file", "path": "src/lib.rs", "params": {"lines": "1-5"}}]"#,
            )
            .unwrap(),
        )
        .unwrap();

    assert_eq!(attachments(&partial), [file(
        "src/lib.rs",
        Some(IndexMap::from([(
            "lines".to_owned(),
            JsonValue(json!("1-5"))
        )]))
    )]);
}

/// The root JSON form of `--cfg` reaches the same attachment a config file's
/// table does.
#[test]
fn an_attachment_in_the_root_json_form_resolves_to_its_url() {
    let mut partial = PartialAppConfig::new_test();
    partial
        .assign(KvAssignment::root_json(json!({
            "conversation": { "attachments": [
                { "type": "file", "path": "src/lib.rs", "params": { "lines": "1-5" } }
            ]}
        })))
        .unwrap();

    let config = build(partial).unwrap();

    assert_eq!(
        config.conversation.attachments[0]
            .to_url()
            .unwrap()
            .as_str(),
        "file://src/lib.rs?lines=1-5"
    );
}

#[test]
fn assigning_one_field_of_a_listed_attachment_keeps_the_others() {
    let mut partial = PartialAppConfig::empty();
    partial
        .assign(
            KvAssignment::try_from_cli(
                "conversation.attachments:",
                r#"[{"type": "file", "path": "src/lib.rs"}]"#,
            )
            .unwrap(),
        )
        .unwrap();

    partial
        .assign(
            KvAssignment::try_from_cli("conversation.attachments.0.path", "src/main.rs").unwrap(),
        )
        .unwrap();

    assert_eq!(attachments(&partial), [file("src/main.rs", None)]);
}

/// A parameter assigned by key joins the ones the attachment already has, as an
/// entry assigned to any other config map does.
#[test]
fn a_param_assigned_by_key_joins_the_ones_already_set() {
    let mut partial = PartialAppConfig::new_test();
    partial
        .assign(
            KvAssignment::try_from_cli(
                "conversation.attachments:",
                r#"[{"type": "file", "path": "src/lib.rs", "params": {"raw": "true"}}]"#,
            )
            .unwrap(),
        )
        .unwrap();

    partial
        .assign(
            KvAssignment::try_from_cli("conversation.attachments.0.params.lines", "1-5").unwrap(),
        )
        .unwrap();

    let config = build(partial).unwrap();
    assert_eq!(
        config.conversation.attachments[0]
            .to_url()
            .unwrap()
            .as_str(),
        "file://src/lib.rs?raw=true&lines=1-5"
    );
}

#[test]
fn a_param_assigned_null_is_removed() {
    let mut partial = PartialAppConfig::new_test();
    partial
        .assign(
            KvAssignment::try_from_cli(
                "conversation.attachments:",
                r#"[{"type": "file", "path": "src/lib.rs", "params": {"raw": "true", "lines": "1-5"}}]"#,
            )
            .unwrap(),
        )
        .unwrap();

    partial
        .assign(
            KvAssignment::try_from_cli("conversation.attachments.0.params.raw:", "null").unwrap(),
        )
        .unwrap();

    let config = build(partial).unwrap();
    assert_eq!(
        config.conversation.attachments[0]
            .to_url()
            .unwrap()
            .as_str(),
        "file://src/lib.rs?lines=1-5"
    );
}

/// A field assigned to a URL attachment edits that part of the URL and keeps
/// the rest, including a parameter given more than once and the fragment.
#[test]
fn a_param_assigned_to_a_url_attachment_keeps_the_rest_of_the_url() {
    let mut partial = PartialAppConfig::new_test();
    partial
        .assign(
            KvAssignment::try_from_cli(
                "conversation.attachments",
                "https://example.com/docs?tag=a&tag=b#intro",
            )
            .unwrap(),
        )
        .unwrap();

    partial
        .assign(KvAssignment::try_from_cli("conversation.attachments.0.params.lang", "en").unwrap())
        .unwrap();

    let config = build(partial).unwrap();
    assert_eq!(
        config.conversation.attachments[0]
            .to_url()
            .unwrap()
            .as_str(),
        "https://example.com/docs?tag=a&tag=b&lang=en#intro"
    );
}

/// An object assigned to a listed URL attachment edits it the way it edits a
/// table: the URL reads as its `type`, `path` and `params`, and the fields the
/// object does not name keep their values.
#[test]
fn an_object_assigned_to_a_url_attachment_keeps_the_fields_it_does_not_name() {
    let mut partial = PartialAppConfig::empty();
    partial
        .assign(
            KvAssignment::try_from_cli("conversation.attachments", "file://src/lib.rs?lines=1-5")
                .unwrap(),
        )
        .unwrap();

    partial
        .assign(
            KvAssignment::try_from_cli("conversation.attachments.0:", r#"{"path": "src/main.rs"}"#)
                .unwrap(),
        )
        .unwrap();

    assert_eq!(attachments(&partial), [file(
        "src/main.rs",
        Some(IndexMap::from([(
            "lines".to_owned(),
            JsonValue(json!("1-5"))
        )]))
    )]);
}

/// The table form always rebuilds `<type>://<path>`, and a handler reads that
/// differently from a URL written without `//`: `gh://pull/544/diff` names a
/// repository owner `pull`, where `gh:pull/544/diff` names pull request 544.
#[test]
fn a_url_without_slashes_after_its_scheme_cannot_be_edited_field_by_field() {
    let mut partial = PartialAppConfig::empty();
    partial
        .assign(KvAssignment::try_from_cli("conversation.attachments", "gh:pull/544/diff").unwrap())
        .unwrap();

    let error = partial
        .assign(
            KvAssignment::try_from_cli("conversation.attachments.0.params.exclude", "docs/**")
                .unwrap(),
        )
        .unwrap_err()
        .downcast::<KvAssignmentError>()
        .unwrap();

    assert_matches!(
        error.error,
        KvAssignmentErrorKind::Parse { error, .. } if error.to_string()
            == "`gh:pull/544/diff` cannot be edited field by field, because it has no `//` \
                after its scheme; set the whole attachment instead"
    );
    assert_eq!(attachments(&partial), [PartialAttachmentConfig::Url(
        "gh:pull/544/diff".parse().unwrap()
    )]);
}
