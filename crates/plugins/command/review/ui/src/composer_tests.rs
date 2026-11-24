use jp_review_api::{AnchorView, Reply, Role, Status};
use pretty_assertions::assert_eq;

use super::*;
use crate::client::Method;

fn range() -> Target {
    Target::Range {
        path: "docs/a.md".to_owned(),
        version: 2,
        bytes: [4, 13],
        excerpt: "is **bold**".to_owned(),
    }
}

fn comment(range: Option<&str>, suggestion: Option<&str>, draft_reply: Option<&str>) -> Comment {
    Comment {
        id: "R-1h4k2m9#3".to_owned(),
        status: Status::Draft,
        role: Role::Reviewer,
        author: "Jane".to_owned(),
        created_at: "2026-11-17T10:00:00Z".parse().unwrap(),
        anchor: AnchorView {
            version: 2,
            path: Some("docs/a.md".to_owned()),
            range: range.map(ToOwned::to_owned),
            projected: None,
            outdated: false,
            resolved: None,
        },
        excerpt: range.map(|_| "is **bold**".to_owned()),
        body: "Name it.".to_owned(),
        body_html: "<p>Name it.</p>\n".to_owned(),
        suggestion: suggestion.map(ToOwned::to_owned),
        replies: vec![],
        draft_reply: draft_reply.map(|body| Reply {
            role: Role::Reviewer,
            author: "Jane".to_owned(),
            created_at: "2026-11-17T10:00:00Z".parse().unwrap(),
            body: body.to_owned(),
            body_html: String::new(),
            verdict: None,
            pass: None,
        }),
        resolved_at: None,
    }
}

/// A suggestion starts from the raw source the range covers, Markdown syntax
/// included.
#[test]
fn a_suggestion_starts_from_the_source_of_its_range() {
    assert_eq!(
        Draft::suggestion(range()).replacement.as_deref(),
        Some("is **bold**")
    );
    assert_eq!(Draft::suggestion(Target::Review).replacement, None);
    assert_eq!(Draft::comment(range()).replacement, None);
}

#[test]
fn a_comment_on_a_range_names_its_version_and_bytes() {
    let draft = Draft {
        body: "Name it.".to_owned(),
        ..Draft::suggestion(range())
    };

    assert_eq!(call("R-1h4k2m9", &draft), Call {
        method: Method::Post,
        url: "/api/reviews/R-1h4k2m9/comments".to_owned(),
        body: Some(
            r#"{"anchor":{"version":2,"path":"docs/a.md","bytes":[4,13]},"body":"Name it.","suggestion":"is **bold**"}"#
                .to_owned()
        ),
    });
}

/// Only a comment on a range can be a suggestion.
#[test]
fn comments_on_the_review_and_a_file_carry_no_suggestion() {
    let on_review = Draft {
        body: "Overall.".to_owned(),
        replacement: Some("stray".to_owned()),
        ..Draft::comment(Target::Review)
    };
    let on_file = Draft {
        body: "File.".to_owned(),
        ..Draft::comment(Target::File {
            path: "docs/a.md".to_owned(),
            version: 3,
        })
    };

    assert_eq!(
        call("R-1h4k2m9", &on_review).body.as_deref(),
        Some(r#"{"body":"Overall."}"#)
    );
    assert_eq!(
        call("R-1h4k2m9", &on_file).body.as_deref(),
        Some(r#"{"anchor":{"version":3,"path":"docs/a.md"},"body":"File."}"#)
    );
}

#[test]
fn an_edit_patches_the_draft_comment() {
    let draft = Draft::edit(&comment(Some("4:1-4:9"), Some("is bold"), None)).unwrap();

    assert_eq!(draft.kind, Kind::Edit {
        number: 3,
        excerpt: Some("is **bold**".to_owned()),
    });
    assert_eq!(call("R-1h4k2m9", &draft), Call {
        method: Method::Patch,
        url: "/api/reviews/R-1h4k2m9/comments/3".to_owned(),
        body: Some(r#"{"body":"Name it.","suggestion":"is bold"}"#.to_owned()),
    });
}

/// A comment on a whole file has no range a suggestion could replace.
#[test]
fn an_edit_of_a_comment_without_a_range_cannot_suggest() {
    let draft = Draft::edit(&comment(None, None, None)).unwrap();

    assert_eq!(draft.kind.excerpt(), None);
    assert_eq!(
        Draft::edit(&comment(Some("4"), None, None))
            .unwrap()
            .kind
            .excerpt(),
        Some("is **bold**")
    );
}

#[test]
fn a_reply_starts_from_the_draft_reply_and_replaces_it() {
    let draft = Draft::reply(&comment(None, None, Some("Still wrong."))).unwrap();

    assert_eq!(draft.body, "Still wrong.");
    assert_eq!(call("R-1h4k2m9", &draft), Call {
        method: Method::Post,
        url: "/api/reviews/R-1h4k2m9/comments/3/replies".to_owned(),
        body: Some(r#"{"body":"Still wrong."}"#.to_owned()),
    });
}

/// A pass sends every comment waiting on the assistant, and a message only when
/// there is one.
#[test]
fn a_pass_sends_everything_pending_with_an_optional_message() {
    let silent = Draft::pass();
    let message = Draft {
        body: "Focus on the intro.".to_owned(),
        ..Draft::pass()
    };

    assert_eq!(call("R-1h4k2m9", &silent), Call {
        method: Method::Post,
        url: "/api/reviews/R-1h4k2m9/passes".to_owned(),
        body: Some("{}".to_owned()),
    });
    assert_eq!(
        call("R-1h4k2m9", &message).body.as_deref(),
        Some(r#"{"message":"Focus on the intro."}"#)
    );
}

#[test]
fn a_draft_needs_something_to_say_before_it_saves() {
    assert_eq!(
        problem(&Draft::comment(range())),
        Some("Write a comment, or suggest a change.")
    );
    assert_eq!(
        problem(&Draft::suggestion(range())),
        None,
        "a bare suggestion"
    );
    assert_eq!(
        problem(&Draft::reply(&comment(None, None, None)).unwrap()),
        Some("Write a reply.")
    );
    assert_eq!(problem(&Draft::pass()), None);
}

/// Local storage keeps a draft as JSON; text it can't read, such as an older
/// UI's, is dropped rather than misread.
#[test]
fn a_draft_round_trips_through_local_storage() {
    let draft = Draft {
        body: "Name it.".to_owned(),
        ..Draft::suggestion(range())
    };
    let stored = encode(&draft);

    assert_eq!(
        stored,
        r#"{"kind":{"comment":{"on":{"range":{"path":"docs/a.md","version":2,"bytes":[4,13],"excerpt":"is **bold**"}}}},"body":"Name it.","replacement":"is **bold**"}"#
    );
    assert_eq!(decode(&stored), Some(draft));
    assert_eq!(decode(r#"{"kind":"poem"}"#), None);
    assert_eq!(key("R-1h4k2m9"), "jp-review:composer:R-1h4k2m9");
}

#[test]
fn the_composer_says_what_it_writes() {
    assert_eq!(
        Kind::Comment { on: range() }.title(),
        "Comment on a passage of docs/a.md, version 2"
    );
    assert_eq!(Kind::Reply { number: 3 }.title(), "Reply to #3");
    assert_eq!(Kind::Pass.save(), "Send");
    assert!(Kind::Comment { on: range() }.can_unsuggest());
    assert!(
        !Kind::Edit {
            number: 3,
            excerpt: None,
        }
        .can_unsuggest()
    );
}
