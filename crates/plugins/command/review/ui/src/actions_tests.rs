use jp_review_api::{AnchorView, Reply};
use pretty_assertions::assert_eq;

use super::*;

fn comment(status: Status, role: Role, replies: &[Role], draft_reply: bool) -> Comment {
    let reply = |role: Role| Reply {
        role,
        author: "Jane".to_owned(),
        created_at: "2026-11-17T10:00:00Z".parse().unwrap(),
        body: "Reply.".to_owned(),
        body_html: "<p>Reply.</p>\n".to_owned(),
        verdict: None,
        pass: None,
    };

    Comment {
        id: "R-1h4k2m9#3".to_owned(),
        status,
        role,
        author: "Jane".to_owned(),
        created_at: "2026-11-17T10:00:00Z".parse().unwrap(),
        anchor: AnchorView {
            version: 1,
            path: None,
            range: None,
            projected: None,
            outdated: false,
            resolved: None,
        },
        excerpt: None,
        body: "Body.".to_owned(),
        body_html: "<p>Body.</p>\n".to_owned(),
        suggestion: None,
        replies: replies.iter().copied().map(reply).collect(),
        draft_reply: draft_reply.then(|| reply(Role::Reviewer)),
        resolved_at: None,
    }
}

#[test]
fn a_card_offers_what_its_comment_allows() {
    let draft = comment(Status::Draft, Role::Reviewer, &[], false);
    let open = comment(Status::Open, Role::Reviewer, &[Role::Assistant], false);
    let replying = comment(Status::Open, Role::Reviewer, &[Role::Assistant], true);
    let resolved = comment(Status::Resolved, Role::Reviewer, &[], false);

    assert_eq!(actions(&draft), [Action::Edit, Action::Withdraw]);
    assert_eq!(actions(&open), [Action::Reply, Action::Resolve]);
    assert_eq!(actions(&replying), [
        Action::EditReply,
        Action::WithdrawReply,
        Action::Resolve
    ]);
    assert_eq!(actions(&resolved), []);
}

#[test]
fn a_comment_is_numbered_by_its_id() {
    let draft = comment(Status::Draft, Role::Reviewer, &[], false);

    assert_eq!(number(&draft), Some(3));
    assert_eq!(
        number(&Comment {
            id: "R-1h4k2m9".to_owned(),
            ..draft
        }),
        None
    );
}

/// The next pass sends drafts, draft replies, and comments the assistant hasn't
/// answered since the reviewer last wrote.
#[test]
fn the_next_pass_sends_what_awaits_the_assistant() {
    let comments = [
        comment(Status::Draft, Role::Reviewer, &[], false),
        comment(Status::Open, Role::Reviewer, &[], false),
        comment(Status::Open, Role::Reviewer, &[Role::Assistant], true),
        comment(Status::Open, Role::Reviewer, &[Role::Assistant], false),
        comment(Status::Open, Role::Assistant, &[], false),
        comment(Status::Resolved, Role::Reviewer, &[], true),
    ];

    let sent: Vec<bool> = comments.iter().map(awaits_assistant).collect();

    assert_eq!(sent, [true, true, true, false, false, false]);
    assert_eq!(pending(&comments), 3);
}
