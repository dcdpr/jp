use jp_review_api::Running;
use pretty_assertions::assert_eq;

use super::*;

#[test]
fn the_detail_asks_for_what_changed_since_a_revision() {
    assert_eq!(
        detail_url("R-1h4k2m9", None, None),
        "/api/reviews/R-1h4k2m9"
    );
    assert_eq!(
        detail_url("R-1h4k2m9", Some(7), Some(3)),
        "/api/reviews/R-1h4k2m9?revision=7&version=3"
    );
    assert_eq!(
        detail_url("R-1h4k2m9", None, Some(3)),
        "/api/reviews/R-1h4k2m9?version=3"
    );
}

#[test]
fn a_file_and_its_diff_name_the_path_encoded() {
    assert_eq!(
        file_url("R-1h4k2m9", 2, "docs/a b&c.md"),
        "/api/reviews/R-1h4k2m9/versions/2/file?path=docs/a%20b%26c.md&format=html"
    );
    assert_eq!(
        diff_url("R-1h4k2m9", Span::Pass(2), "docs/a.md"),
        "/api/reviews/R-1h4k2m9/diff?path=docs/a.md&pass=2"
    );
    assert_eq!(
        diff_url(
            "R-1h4k2m9",
            Span::Versions {
                from: Some(1),
                to: Some(3),
            },
            "docs/a.md"
        ),
        "/api/reviews/R-1h4k2m9/diff?path=docs/a.md&from=1&to=3"
    );
    assert_eq!(
        diff_url(
            "R-1h4k2m9",
            Span::Versions {
                from: None,
                to: None,
            },
            "docs/a.md"
        ),
        "/api/reviews/R-1h4k2m9/diff?path=docs/a.md"
    );
}

#[test]
fn the_write_endpoints_name_the_review_and_comment() {
    assert_eq!(comments_url("R-1h4k2m9"), "/api/reviews/R-1h4k2m9/comments");
    assert_eq!(
        comment_url("R-1h4k2m9", 3),
        "/api/reviews/R-1h4k2m9/comments/3"
    );
    assert_eq!(
        replies_url("R-1h4k2m9", 3),
        "/api/reviews/R-1h4k2m9/comments/3/replies"
    );
    assert_eq!(
        resolve_url("R-1h4k2m9", 3),
        "/api/reviews/R-1h4k2m9/comments/3/resolve"
    );
    assert_eq!(
        withdraw_url("R-1h4k2m9", 3, Draft::Reply),
        "/api/reviews/R-1h4k2m9/comments/3?draft=reply"
    );
    assert_eq!(
        reopen_comment_url("R-1h4k2m9", 3),
        "/api/reviews/R-1h4k2m9/comments/3/reopen"
    );
    assert_eq!(passes_url("R-1h4k2m9"), "/api/reviews/R-1h4k2m9/passes");
    assert_eq!(stop_url("R-1h4k2m9"), "/api/reviews/R-1h4k2m9/stop");
    assert_eq!(versions_url("R-1h4k2m9"), "/api/reviews/R-1h4k2m9/versions");
}

#[test]
fn approving_reading_and_reopening_name_the_review() {
    assert_eq!(
        approvals_url("R-1h4k2m9"),
        "/api/reviews/R-1h4k2m9/approvals"
    );
    assert_eq!(
        approval_url("R-1h4k2m9", 2),
        "/api/reviews/R-1h4k2m9/approvals/2"
    );
    assert_eq!(read_url("R-1h4k2m9"), "/api/reviews/R-1h4k2m9/read");
    assert_eq!(
        unread_url("R-1h4k2m9", "docs/a b&c.md"),
        "/api/reviews/R-1h4k2m9/read?path=docs/a%20b%26c.md"
    );
    assert_eq!(reopen_url("R-1h4k2m9"), "/api/reviews/R-1h4k2m9/reopen");
}

#[test]
fn a_put_carries_its_body_as_json() {
    assert_eq!(Call::put(read_url("R-1h4k2m9"), &[1, 2]), Call {
        method: Method::Put,
        url: "/api/reviews/R-1h4k2m9/read".to_owned(),
        body: Some("[1,2]".to_owned()),
    });
}

#[test]
fn a_call_without_a_body_sends_none() {
    assert_eq!(Call::delete(comment_url("R-1h4k2m9", 3)), Call {
        method: Method::Delete,
        url: "/api/reviews/R-1h4k2m9/comments/3".to_owned(),
        body: None,
    });
    assert_eq!(Call::post_empty(versions_url("R-1h4k2m9")).body, None);
}

#[test]
fn an_answer_carries_its_value_or_the_reason_it_was_refused() {
    assert_eq!(
        read::<Running>(200, r#"{"number":2,"stoppable":true}"#),
        Ok(Running {
            number: 2,
            conversation: None,
            stoppable: true,
        })
    );
    assert_eq!(
        read::<Running>(404, r#"{"error":"No review R-1h4k2m9."}"#),
        Err("No review R-1h4k2m9.".to_owned())
    );
    assert_eq!(
        read::<Running>(502, "Bad Gateway"),
        Err("The server answered 502.".to_owned())
    );
    assert!(
        read::<Running>(200, "{")
            .unwrap_err()
            .starts_with("The server answered with what this page can't read: ")
    );
}
