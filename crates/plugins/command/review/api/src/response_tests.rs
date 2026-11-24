use std::fmt::Debug;

use pretty_assertions::assert_eq;
use serde::{Serialize, de::DeserializeOwned};

use super::*;

/// Assert that `value` is written as `json` exactly, and read back from it.
fn pinned<T: Serialize + DeserializeOwned + PartialEq + Debug>(value: &T, json: &str) {
    assert_eq!(serde_json::to_string(value).unwrap(), json);
    assert_eq!(&serde_json::from_str::<T>(json).unwrap(), value);
}

fn at(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text).unwrap().to_utc()
}

fn head() -> Head {
    Head {
        id: "R-1h4k2m9".to_owned(),
        title: "Notes".to_owned(),
        status: State::Open,
        author: "Jane Doe <jane@example.com>".to_owned(),
        created_at: at("2026-11-17T10:00:00Z"),
        closed_at: None,
        files: vec!["docs/notes.md".to_owned()],
    }
}

#[test]
fn a_listing_names_its_workspace_and_its_reviews() {
    pinned(
        &Listing {
            workspace: "jp".to_owned(),
            reviews: vec![Summary {
                head: head(),
                latest: 3,
                base: Some(1),
                comments: Counts {
                    draft: 1,
                    open: 2,
                    resolved: 0,
                },
                running: Some(2),
            }],
            unreadable: vec![Unreadable {
                id: "R-0000000".to_owned(),
                error: "review.json: EOF".to_owned(),
            }],
        },
        concat!(
            r#"{"workspace":"jp","reviews":[{"id":"R-1h4k2m9","title":"Notes","status":"open","#,
            r#""author":"Jane Doe <jane@example.com>","created_at":"2026-11-17T10:00:00Z","#,
            r#""files":["docs/notes.md"],"latest":3,"base":1,"#,
            r#""comments":{"draft":1,"open":2,"resolved":0},"running":2}],"#,
            r#""unreadable":[{"id":"R-0000000","error":"review.json: EOF"}]}"#,
        ),
    );
}

/// A poll by a caller holding the revision carries no review, and still says
/// who runs a pass and whether the files changed.
#[test]
fn a_detail_reply_leaves_out_a_review_the_caller_holds() {
    pinned(
        &DetailReply {
            workspace: "jp".to_owned(),
            revision: 4,
            running: Some(Running {
                number: 2,
                conversation: Some("jp-c17629384710".to_owned()),
                stoppable: true,
            }),
            working_tree_changed: false,
            review: None,
        },
        concat!(
            r#"{"workspace":"jp","revision":4,"#,
            r#""running":{"number":2,"conversation":"jp-c17629384710","stoppable":true},"#,
            r#""working_tree_changed":false}"#,
        ),
    );
    pinned(
        &DetailReply {
            workspace: "jp".to_owned(),
            revision: 4,
            running: None,
            working_tree_changed: true,
            review: None,
        },
        r#"{"workspace":"jp","revision":4,"running":null,"working_tree_changed":true}"#,
    );
}

#[test]
fn a_detail_carries_versions_passes_and_placed_comments() {
    pinned(
        &Detail {
            head: head(),
            revision: 9,
            version: 3,
            versions: vec![Version {
                number: 3,
                label: None,
                created_at: at("2026-11-17T10:04:00Z"),
                files: BTreeMap::from([
                    ("docs/notes.md".to_owned(), Some("ab12".to_owned())),
                    ("docs/gone.md".to_owned(), None),
                ]),
            }],
            conversation: Some("jp-c17629384710".to_owned()),
            passes: vec![Pass {
                number: 1,
                author: "Jane".to_owned(),
                created_at: at("2026-11-17T10:01:00Z"),
                version: 2,
                comments: vec![1],
                message: Some("First half.".to_owned()),
                conversation: Some("jp-c17629384710".to_owned()),
                ended: Some(Ended {
                    outcome: Outcome::Completed,
                    at: at("2026-11-17T10:04:00Z"),
                    version: Some(3),
                    error: None,
                    answer: Some("Fixed both.".to_owned()),
                }),
            }],
            comments: vec![Comment {
                id: "R-1h4k2m9#1".to_owned(),
                status: Status::Open,
                role: Role::Reviewer,
                author: "Jane".to_owned(),
                created_at: at("2026-11-17T10:00:30Z"),
                anchor: AnchorView {
                    version: 2,
                    path: Some("docs/notes.md".to_owned()),
                    range: Some("3:1-3:6".to_owned()),
                    projected: None,
                    outdated: true,
                    resolved: Some(Placed {
                        version: 2,
                        range: Some("3:1-3:6".to_owned()),
                        bytes: Some([9, 14]),
                    }),
                },
                excerpt: Some("First".to_owned()),
                body: "Name it.".to_owned(),
                body_html: "<p>Name it.</p>\n".to_owned(),
                suggestion: Some("Opening".to_owned()),
                replies: vec![Reply {
                    role: Role::Assistant,
                    author: "jp".to_owned(),
                    created_at: at("2026-11-17T10:04:00Z"),
                    body: "Done.".to_owned(),
                    body_html: "<p>Done.</p>\n".to_owned(),
                    verdict: Some(Verdict::Accept),
                    pass: Some(1),
                }],
                draft_reply: Some(Reply {
                    role: Role::Reviewer,
                    author: "Jane".to_owned(),
                    created_at: at("2026-11-17T10:05:00Z"),
                    body: "Thanks.".to_owned(),
                    body_html: "<p>Thanks.</p>\n".to_owned(),
                    verdict: None,
                    pass: None,
                }),
                resolved_at: None,
            }],
        },
        concat!(
            r#"{"id":"R-1h4k2m9","title":"Notes","status":"open","#,
            r#""author":"Jane Doe <jane@example.com>","created_at":"2026-11-17T10:00:00Z","#,
            r#""files":["docs/notes.md"],"revision":9,"version":3,"#,
            r#""versions":[{"number":3,"created_at":"2026-11-17T10:04:00Z","#,
            r#""files":{"docs/gone.md":null,"docs/notes.md":"ab12"}}],"#,
            r#""conversation":"jp-c17629384710","#,
            r#""passes":[{"number":1,"author":"Jane","created_at":"2026-11-17T10:01:00Z","#,
            r#""version":2,"comments":[1],"message":"First half.","#,
            r#""conversation":"jp-c17629384710","#,
            r#""ended":{"outcome":"completed","at":"2026-11-17T10:04:00Z","version":3,"#,
            r#""answer":"Fixed both."}}],"#,
            r#""comments":[{"id":"R-1h4k2m9#1","status":"open","role":"reviewer","#,
            r#""author":"Jane","created_at":"2026-11-17T10:00:30Z","#,
            r#""anchor":{"version":2,"path":"docs/notes.md","range":"3:1-3:6","outdated":true,"#,
            r#""resolved":{"version":2,"range":"3:1-3:6","bytes":[9,14]}},"#,
            r#""excerpt":"First","body":"Name it.","body_html":"<p>Name it.</p>\n","#,
            r#""suggestion":"Opening","#,
            r#""replies":[{"role":"assistant","author":"jp","created_at":"2026-11-17T10:04:00Z","#,
            r#""body":"Done.","body_html":"<p>Done.</p>\n","verdict":"accept","pass":1}],"#,
            r#""draft_reply":{"role":"reviewer","author":"Jane","created_at":"2026-11-17T10:05:00Z","#,
            r#""body":"Thanks.","body_html":"<p>Thanks.</p>\n"}}]}"#,
        ),
    );
}

/// A comment on the review as a whole names neither a file nor a range.
#[test]
fn an_anchor_on_the_review_carries_only_versions() {
    pinned(
        &AnchorView {
            version: 1,
            path: None,
            range: None,
            projected: Some(Placed {
                version: 1,
                range: None,
                bytes: None,
            }),
            outdated: false,
            resolved: None,
        },
        r#"{"version":1,"projected":{"version":1},"outdated":false}"#,
    );
}

#[test]
fn a_file_carries_its_source_and_maybe_its_html() {
    pinned(
        &FileReply {
            review: "R-1h4k2m9".to_owned(),
            version: 1,
            path: "docs/b.txt".to_owned(),
            source: "plain\n".to_owned(),
            html: Some("<pre><span data-s=\"0\" data-e=\"5\">plain</span>\n</pre>\n".to_owned()),
        },
        concat!(
            r#"{"review":"R-1h4k2m9","version":1,"path":"docs/b.txt","source":"plain\n","#,
            r#""html":"<pre><span data-s=\"0\" data-e=\"5\">plain</span>\n</pre>\n"}"#,
        ),
    );
}

#[test]
fn a_diff_places_its_changes_in_the_later_version() {
    pinned(
        &DiffView {
            review: "R-1h4k2m9".to_owned(),
            from: 2,
            to: 3,
            files: vec![FileDiff {
                path: "docs/notes.md".to_owned(),
                change: Change::Modified,
                diff: "@@ -40,3 +40,3 @@\n".to_owned(),
                added: vec![[4113, 4148]],
                removed: vec![3980],
            }],
        },
        concat!(
            r#"{"review":"R-1h4k2m9","from":2,"to":3,"files":[{"path":"docs/notes.md","#,
            r#""change":"modified","diff":"@@ -40,3 +40,3 @@\n","added":[[4113,4148]],"#,
            r#""removed":[3980]}]}"#,
        ),
    );
}

#[test]
fn the_comment_actions_answer_what_they_did() {
    pinned(
        &Withdrawal {
            id: "R-1h4k2m9#1".to_owned(),
            withdrawn: Draft::Reply,
        },
        r#"{"id":"R-1h4k2m9#1","withdrawn":"reply"}"#,
    );
    pinned(
        &Resolved {
            id: "R-1h4k2m9#1".to_owned(),
            outcome: Resolution::AlreadyResolved,
        },
        r#"{"id":"R-1h4k2m9#1","outcome":"already_resolved"}"#,
    );
}

#[test]
fn the_pass_actions_answer_what_they_did() {
    pinned(
        &Started {
            review: "R-1h4k2m9".to_owned(),
            pass: Pass {
                number: 2,
                author: "Jane".to_owned(),
                created_at: at("2026-11-17T10:01:00Z"),
                version: 3,
                comments: vec![],
                message: None,
                conversation: None,
                ended: None,
            },
        },
        r#"{"review":"R-1h4k2m9","number":2,"author":"Jane","created_at":"2026-11-17T10:01:00Z","version":3}"#,
    );
    pinned(
        &Stopping {
            review: "R-1h4k2m9".to_owned(),
            number: 2,
            stopping: true,
        },
        r#"{"review":"R-1h4k2m9","number":2,"stopping":true}"#,
    );
    pinned(
        &Recorded {
            review: "R-1h4k2m9".to_owned(),
            version: 4,
            recorded: true,
        },
        r#"{"review":"R-1h4k2m9","version":4,"recorded":true}"#,
    );
}

#[test]
fn an_error_is_its_message() {
    pinned(
        &ErrorBody {
            error: "No review R-1h4k2m9.".to_owned(),
        },
        r#"{"error":"No review R-1h4k2m9."}"#,
    );
}

/// A client reads what a newer server adds without failing.
#[test]
fn an_answer_with_a_field_this_crate_doesnt_know_still_reads() {
    let running: Running =
        serde_json::from_str(r#"{"number":2,"stoppable":false,"since":"later"}"#).unwrap();

    assert_eq!(running, Running {
        number: 2,
        conversation: None,
        stoppable: false,
    });
}

#[test]
fn the_vocabulary_is_written_in_snake_case() {
    pinned(&State::Closed, r#""closed""#);
    pinned(&Status::Resolved, r#""resolved""#);
    pinned(&Role::Assistant, r#""assistant""#);
    pinned(&Verdict::Defer, r#""defer""#);
    pinned(&Outcome::Interrupted, r#""interrupted""#);
    pinned(&Change::Deleted, r#""deleted""#);
    pinned(&Draft::Comment, r#""comment""#);
    pinned(&Resolution::Resolved, r#""resolved""#);
}
