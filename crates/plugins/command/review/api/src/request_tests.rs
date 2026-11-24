use std::fmt::Debug;

use pretty_assertions::assert_eq;
use serde::{Serialize, de::DeserializeOwned};

use super::*;

/// Assert that `value` is written as `json` exactly, and read back from it.
fn pinned<T: Serialize + DeserializeOwned + PartialEq + Debug>(value: &T, json: &str) {
    assert_eq!(serde_json::to_string(value).unwrap(), json);
    assert_eq!(&serde_json::from_str::<T>(json).unwrap(), value);
}

#[test]
fn a_new_review_names_its_files_and_maybe_a_title() {
    pinned(
        &NewReview {
            files: vec!["docs/a.md".to_owned()],
            title: Some("Notes".to_owned()),
        },
        r#"{"files":["docs/a.md"],"title":"Notes"}"#,
    );
    pinned(
        &NewReview {
            files: vec!["docs/a.md".to_owned()],
            title: None,
        },
        r#"{"files":["docs/a.md"]}"#,
    );
}

#[test]
fn a_new_comment_anchors_to_bytes_or_a_range() {
    pinned(
        &NewCommentRequest {
            anchor: Some(AnchorRequest {
                version: Some(3),
                path: Some("docs/a.md".to_owned()),
                range: None,
                bytes: Some([9, 14]),
            }),
            body: "Name it.".to_owned(),
            suggestion: None,
        },
        r#"{"anchor":{"version":3,"path":"docs/a.md","bytes":[9,14]},"body":"Name it."}"#,
    );
    pinned(
        &NewCommentRequest {
            anchor: Some(AnchorRequest {
                version: Some(3),
                path: Some("docs/a.md".to_owned()),
                range: Some("42:5-43:18".to_owned()),
                bytes: None,
            }),
            body: String::new(),
            suggestion: Some(String::new()),
        },
        r#"{"anchor":{"version":3,"path":"docs/a.md","range":"42:5-43:18"},"body":"","suggestion":""}"#,
    );
}

/// A comment on the review as a whole names nothing but its body.
#[test]
fn a_new_comment_reads_with_only_a_body() {
    assert_eq!(
        serde_json::from_str::<NewCommentRequest>(r#"{"body":"Too long.","anchor":null}"#).unwrap(),
        NewCommentRequest {
            anchor: None,
            body: "Too long.".to_owned(),
            suggestion: None,
        }
    );
}

#[test]
fn a_request_naming_a_field_the_endpoint_doesnt_take_is_refused() {
    let error =
        serde_json::from_str::<ReplyRequest>(r#"{"body":"Why?","verdict":"ask"}"#).unwrap_err();

    assert!(
        error.to_string().starts_with("unknown field `verdict`"),
        "{error}"
    );
}

#[test]
fn an_edit_names_only_what_changes() {
    pinned(
        &EditRequest {
            anchor: None,
            body: Some("Rename it.".to_owned()),
            suggestion: None,
        },
        r#"{"body":"Rename it."}"#,
    );
}

#[test]
fn a_reply_is_its_body() {
    pinned(
        &ReplyRequest {
            body: "Why?".to_owned(),
        },
        r#"{"body":"Why?"}"#,
    );
}

#[test]
fn a_pass_request_is_empty_by_default() {
    pinned(&PassRequest::default(), "{}");
    pinned(
        &PassRequest {
            only: vec![1, 2],
            message: Some("First half only.".to_owned()),
            conversation: Some("jp-c17629384710".to_owned()),
        },
        r#"{"only":[1,2],"message":"First half only.","conversation":"jp-c17629384710"}"#,
    );
}

#[test]
fn a_stop_may_name_its_pass() {
    pinned(&StopRequest::default(), "{}");
    pinned(&StopRequest { pass: Some(2) }, r#"{"pass":2}"#);
}

#[test]
fn the_queries_name_their_parameters() {
    pinned(
        &DetailQuery {
            revision: Some(7),
            version: Some(3),
        },
        r#"{"revision":7,"version":3}"#,
    );
    pinned(
        &FileQuery {
            path: "docs/a.md".to_owned(),
            format: Format::Html,
        },
        r#"{"path":"docs/a.md","format":"html"}"#,
    );
    pinned(
        &DiffQuery {
            from: Some(1),
            to: Some(3),
            pass: None,
            path: Some("docs/a.md".to_owned()),
        },
        r#"{"from":1,"to":3,"path":"docs/a.md"}"#,
    );
    pinned(
        &DiffQuery {
            pass: Some(2),
            ..DiffQuery::default()
        },
        r#"{"pass":2}"#,
    );
}

#[test]
fn a_file_is_asked_for_as_text_by_default() {
    assert_eq!(
        serde_json::from_str::<FileQuery>(r#"{"path":"docs/a.md"}"#).unwrap(),
        FileQuery {
            path: "docs/a.md".to_owned(),
            format: Format::Text,
        }
    );
}
