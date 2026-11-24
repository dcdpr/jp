use chrono::TimeZone as _;
use pretty_assertions::assert_eq;

use super::*;
use crate::{pass::NewPass, text::Range};

fn at(minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 17, 10, minute, 0).unwrap()
}

fn id() -> ReviewId {
    "R-1h4k2m9".parse().unwrap()
}

/// A review of one file, with one version.
fn review() -> Review {
    let mut review = Review::new(
        "Review of a.md".to_owned(),
        "Jane Doe <jane@example.com>".to_owned(),
        at(0),
        vec!["a.md".to_owned()],
    );
    review.record(
        Files::from([("a.md".to_owned(), Some("aaaa".to_owned()))]),
        None,
        at(0),
    );

    review
}

fn on_review(body: &str) -> NewComment {
    NewComment {
        role: Role::Reviewer,
        author: "Jane Doe <jane@example.com>".to_owned(),
        created_at: at(1),
        content: Content {
            anchor: Anchor {
                version: 1,
                target: Target::Review,
            },
            body: body.to_owned(),
            suggestion: None,
        },
    }
}

fn on_range(range: &str, suggestion: Option<&str>) -> NewComment {
    NewComment {
        content: Content {
            anchor: Anchor {
                version: 1,
                target: Target::Range("a.md".to_owned(), range.parse::<Range>().unwrap()),
            },
            body: String::new(),
            suggestion: suggestion.map(ToOwned::to_owned),
        },
        ..on_review("")
    }
}

fn reply_from(role: Role, verdict: Option<Verdict>) -> Reply {
    Reply {
        role,
        author: "jp".to_owned(),
        created_at: at(2),
        body: "Done.".to_owned(),
        verdict,
        pass: None,
    }
}

fn refused(number: u32, why: Why) -> Refusal {
    Refusal::Comment(number, why)
}

#[test]
fn a_reviewer_comment_starts_as_a_draft() {
    let mut review = review();

    assert_eq!(review.add_comment(on_review("First.")), Ok(1));
    assert_eq!(review.add_comment(on_review("Second.")), Ok(2));

    assert_eq!(review.comment(2).unwrap().status(), Status::Draft);
    assert_eq!(review.last_comment, 2);
}

#[test]
fn an_assistant_comment_is_open() {
    let mut review = review();

    review
        .add_comment(NewComment {
            role: Role::Assistant,
            ..on_review("You missed this.")
        })
        .unwrap();

    assert_eq!(review.comment(1).unwrap().status(), Status::Open);
}

#[test]
fn a_withdrawn_comment_retires_its_number() {
    let mut review = review();
    for body in ["One.", "Two.", "Three."] {
        review.add_comment(on_review(body)).unwrap();
    }

    assert_eq!(review.withdraw(3), Ok(Draft::Comment));
    assert_eq!(review.comment(3), Err(refused(3, Why::Withdrawn)));
    assert_eq!(review.comment(9), Err(refused(9, Why::Missing)));
    assert_eq!(review.add_comment(on_review("Four.")), Ok(4));

    review.withdraw(4).unwrap();
    review.withdraw(2).unwrap();
    assert_eq!(review.add_comment(on_review("Five.")), Ok(5));
}

#[test]
fn a_comment_needs_a_body_or_a_suggestion_on_a_range() {
    let mut review = review();

    let mut on_the_review = on_review("Replace it.");
    on_the_review.content.suggestion = Some("text".to_owned());

    assert_eq!(review.add_comment(on_review("  ")), Err(Refusal::EmptyBody));
    assert_eq!(
        review.add_comment(on_the_review),
        Err(Refusal::SuggestionWithoutRange)
    );
    assert_eq!(
        review.add_comment(on_range("2", None)),
        Err(Refusal::EmptyBody)
    );

    // An empty suggestion suggests deleting the range.
    assert_eq!(review.add_comment(on_range("2", Some(""))), Ok(1));
    assert_eq!(review.add_comment(on_range("2:1-2:4", Some("Two"))), Ok(2));
    assert!(
        review
            .comments
            .iter()
            .all(|comment| comment.content.body.is_empty())
    );
}

#[test]
fn a_draft_takes_no_reply_and_is_not_resolved() {
    let mut review = review();
    review.add_comment(on_review("First.")).unwrap();

    assert_eq!(
        review.reply(1, "Jane".to_owned(), at(3), "Reply.".to_owned()),
        Err(refused(1, Why::Draft))
    );
    assert_eq!(review.resolve(1, at(3)), Err(refused(1, Why::Draft)));
}

#[test]
fn a_sent_comment_holds_one_draft_reply_at_a_time() {
    let mut review = review();
    review.add_comment(on_review("First.")).unwrap();
    review.comments[0].pass = Some(1);

    assert_eq!(
        review.reply(1, "Jane".to_owned(), at(3), "Sure?".to_owned()),
        Ok(Replied::Added)
    );
    assert_eq!(
        review.reply(1, "Jane".to_owned(), at(4), "Are you sure?".to_owned()),
        Ok(Replied::Replaced)
    );
    assert_eq!(
        review.reply(1, "Jane".to_owned(), at(4), " ".to_owned()),
        Err(Refusal::EmptyBody)
    );

    let draft = review.comment(1).unwrap().draft_reply.clone().unwrap();
    assert_eq!(
        (draft.body.as_str(), draft.created_at),
        ("Are you sure?", at(4))
    );
    assert_eq!(review.comment(1).unwrap().status(), Status::Open);
}

#[test]
fn editing_changes_a_draft_comment() {
    let mut review = review();
    review.add_comment(on_range("2", Some("two"))).unwrap();
    let anchor = Anchor {
        version: 1,
        target: Target::Range("a.md".to_owned(), "3".parse().unwrap()),
    };

    let edited = review.edit(1, Edit {
        anchor: Some(anchor.clone()),
        suggestion: Some("three".to_owned()),
        body: Some("Better.".to_owned()),
    });

    assert_eq!(edited, Ok(Draft::Comment));
    assert_eq!(review.comment(1).unwrap().content, Content {
        anchor,
        body: "Better.".to_owned(),
        suggestion: Some("three".to_owned()),
    });
}

/// The whole comment is checked after the edit, so an anchor that drops the
/// range a suggestion needs is refused.
#[test]
fn an_edit_leaving_a_suggestion_without_a_range_is_refused() {
    let mut review = review();
    review.add_comment(on_range("2", Some("two"))).unwrap();

    let edited = review.edit(1, Edit {
        anchor: Some(Anchor {
            version: 1,
            target: Target::File("a.md".to_owned()),
        }),
        ..Edit::default()
    });

    assert_eq!(edited, Err(Refusal::SuggestionWithoutRange));
    assert_eq!(
        review.comment(1).unwrap().content.anchor.target,
        Target::Range("a.md".to_owned(), "2".parse().unwrap())
    );
}

#[test]
fn editing_a_sent_comment_changes_only_its_draft_reply() {
    let mut review = review();
    review.add_comment(on_review("First.")).unwrap();
    review.comments[0].pass = Some(1);
    let body = |text: &str| Edit {
        body: Some(text.to_owned()),
        ..Edit::default()
    };

    assert_eq!(review.edit(1, body("Changed.")), Err(refused(1, Why::Sent)));

    review
        .reply(1, "Jane".to_owned(), at(3), "Sure?".to_owned())
        .unwrap();
    assert_eq!(review.edit(1, body("Certain?")), Ok(Draft::Reply));
    assert_eq!(
        review.edit(1, Edit {
            suggestion: Some("x".to_owned()),
            ..body("Certain?")
        }),
        Err(refused(1, Why::ReplyAnchor))
    );
    assert_eq!(
        review.edit(1, Edit::default()),
        Err(Refusal::NothingToChange)
    );

    let comment = review.comment(1).unwrap();
    assert_eq!(comment.content.body, "First.");
    assert_eq!(comment.draft_reply.as_ref().unwrap().body, "Certain?");
}

#[test]
fn withdrawing_a_draft_reply_keeps_the_comment() {
    let mut review = review();
    review.add_comment(on_review("First.")).unwrap();
    review.comments[0].pass = Some(1);

    assert_eq!(review.withdraw(1), Err(refused(1, Why::Sent)));

    review
        .reply(1, "Jane".to_owned(), at(3), "Sure?".to_owned())
        .unwrap();
    assert_eq!(review.withdraw(1), Ok(Draft::Reply));
    assert_eq!(review.comment(1).unwrap().draft_reply, None);
}

#[test]
fn resolving_needs_a_sent_comment_without_a_draft_reply() {
    let mut review = review();
    review.add_comment(on_review("First.")).unwrap();
    review.comments[0].pass = Some(1);
    review
        .reply(1, "Jane".to_owned(), at(3), "Sure?".to_owned())
        .unwrap();

    assert_eq!(review.resolve(1, at(4)), Err(refused(1, Why::DraftReply)));

    review.withdraw(1).unwrap();
    assert_eq!(review.resolve(1, at(4)), Ok(Resolution::Resolved));
    assert_eq!(review.resolve(1, at(5)), Ok(Resolution::AlreadyResolved));
    assert_eq!(review.comment(1).unwrap().resolved_at, Some(at(4)));
    assert_eq!(review.comment(1).unwrap().status(), Status::Resolved);
    assert_eq!(
        review.reply(1, "Jane".to_owned(), at(5), "More.".to_owned()),
        Err(refused(1, Why::Resolved))
    );
}

/// The assistant never answered comment 1, so the reviewer wrote in it last.
#[test]
fn a_resolved_comment_is_opened_again() {
    let mut review = review();
    review.add_comment(on_review("First.")).unwrap();
    review.add_comment(on_review("Second.")).unwrap();
    review.comments[0].pass = Some(1);
    review.resolve(1, at(4)).unwrap();

    assert_eq!(review.comment_mut(1).unwrap().reopen(), Ok(true));

    let reopened = review.comment(1).unwrap();
    assert_eq!(reopened.status(), Status::Open);
    assert_eq!(reopened.resolved_at, None);
    assert!(reopened.awaits_assistant(), "the next pass sends it");
    assert_eq!(
        review.reply(1, "Jane".to_owned(), at(5), "Still wrong.".to_owned()),
        Ok(Replied::Added)
    );
}

#[test]
fn only_a_resolved_comment_is_opened_again() {
    let mut review = review();
    review.add_comment(on_review("First.")).unwrap();
    review.add_comment(on_review("Second.")).unwrap();
    review.comments[0].pass = Some(1);

    assert_eq!(review.comment_mut(1).unwrap().reopen(), Ok(false));
    assert_eq!(
        review.comment_mut(2).unwrap().reopen(),
        Err(refused(2, Why::Draft))
    );
    assert_eq!(
        review.comment_mut(3).map(|_| ()),
        Err(refused(3, Why::Missing))
    );
}

#[test]
fn the_assistant_answers_a_reviewer_comment_with_a_verdict() {
    let mut review = review();
    review.add_comment(on_review("First.")).unwrap();

    assert_eq!(
        review.answer(1, reply_from(Role::Assistant, Some(Verdict::Accept))),
        Err(refused(1, Why::Draft))
    );

    review.comments[0].pass = Some(1);
    assert_eq!(
        review.answer(1, reply_from(Role::Assistant, None)),
        Err(refused(1, Why::VerdictRequired))
    );
    assert_eq!(
        review.answer(1, Reply {
            body: " ".to_owned(),
            ..reply_from(Role::Assistant, Some(Verdict::Accept))
        }),
        Err(Refusal::EmptyBody)
    );
    assert_eq!(
        review.answer(1, reply_from(Role::Assistant, Some(Verdict::Accept))),
        Ok(())
    );

    let replies = &review.comment(1).unwrap().replies;
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].verdict, Some(Verdict::Accept));
    assert_eq!(replies[0].pass, None);
}

/// The assistant opened the comment, so the reply is part of a conversation
/// about its own point, not a verdict on the reviewer's.
#[test]
fn the_assistant_answers_its_own_comment_without_a_verdict() {
    let mut review = review();
    review
        .add_comment(NewComment {
            role: Role::Assistant,
            ..on_review("You missed this.")
        })
        .unwrap();

    assert_eq!(
        review.answer(1, reply_from(Role::Assistant, Some(Verdict::Ask))),
        Err(refused(1, Why::VerdictRefused))
    );
    assert_eq!(review.answer(1, reply_from(Role::Assistant, None)), Ok(()));

    review.close(at(9)).unwrap();
    assert_eq!(
        review.answer(1, reply_from(Role::Assistant, None)),
        Err(Refusal::Closed)
    );
}

#[test]
fn a_comment_awaits_the_assistant_until_the_assistant_writes_last() {
    let mut review = review();
    review.add_comment(on_review("First.")).unwrap();
    let waiting = |review: &Review| review.comment(1).unwrap().awaits_assistant();

    // A draft.
    assert!(waiting(&review));

    // Sent, and not answered.
    review.comments[0].pass = Some(1);
    assert!(waiting(&review));

    review.comments[0]
        .replies
        .push(reply_from(Role::Assistant, Some(Verdict::Ask)));
    assert!(!waiting(&review));

    review
        .reply(1, "Jane".to_owned(), at(3), "This.".to_owned())
        .unwrap();
    assert!(waiting(&review));

    review.withdraw(1).unwrap();
    review.resolve(1, at(4)).unwrap();
    assert!(!waiting(&review));
}

#[test]
fn what_is_unanswered_is_what_the_reviewer_wrote_since_the_assistant() {
    let mut review = review();
    review.add_comment(on_review("First.")).unwrap();
    review
        .add_comment(NewComment {
            role: Role::Assistant,
            ..on_review("Mine.")
        })
        .unwrap();
    let reviewer = reply_from(Role::Reviewer, None);
    let assistant = reply_from(Role::Assistant, None);

    assert_eq!(review.comment(1).unwrap().unanswered(), (true, &[][..]));
    assert_eq!(review.comment(2).unwrap().unanswered(), (false, &[][..]));

    review.comments[0].replies = vec![reviewer.clone()];
    review.comments[1].replies = vec![reviewer.clone()];
    assert_eq!(
        review.comment(1).unwrap().unanswered(),
        (true, &[reviewer.clone()][..])
    );
    assert_eq!(
        review.comment(2).unwrap().unanswered(),
        (false, &[reviewer.clone()][..])
    );

    review.comments[0].replies = vec![reviewer.clone(), assistant, reviewer.clone()];
    assert_eq!(
        review.comment(1).unwrap().unanswered(),
        (false, &[reviewer][..])
    );
}

#[test]
fn accepted_lists_open_comments_whose_last_reply_accepts_them() {
    let mut review = review();
    for body in ["1", "2", "3", "4", "5", "6"] {
        review.add_comment(on_review(body)).unwrap();
    }
    for comment in &mut review.comments {
        comment.pass = Some(1);
    }
    let accept = reply_from(Role::Assistant, Some(Verdict::Accept));

    // 1 is accepted. 2 was accepted, then the reviewer pushed back. 3 was
    // declined. 4 holds a draft reply. 5 is resolved. 6 has no reply.
    review.comments[0].replies = vec![accept.clone()];
    review.comments[1].replies = vec![accept.clone(), reply_from(Role::Reviewer, None)];
    review.comments[2].replies = vec![reply_from(Role::Assistant, Some(Verdict::Decline))];
    review.comments[3].replies = vec![accept.clone()];
    review.comments[3].draft_reply = Some(reply_from(Role::Reviewer, None));
    review.comments[4].replies = vec![accept];
    review.comments[4].resolved_at = Some(at(5));

    assert_eq!(review.accepted(), [1]);
}

/// The assistant answering a running pass could no longer write its replies.
#[test]
fn a_review_with_a_running_pass_stays_open() {
    let mut review = review();
    review.add_comment(on_review("First.")).unwrap();
    review
        .submit(NewPass {
            author: "Jane".to_owned(),
            created_at: at(2),
            version: 1,
            only: vec![],
            message: None,
            conversation: None,
        })
        .unwrap();

    assert_eq!(review.close(at(3)), Err(Refusal::PassRunning(1)));
    assert_eq!(review.state(), State::Open);

    review.passes.interrupt(at(4));
    assert_eq!(review.close(at(5)), Ok(true));
}

#[test]
fn a_closed_review_refuses_every_change() {
    let mut review = review();
    review.add_comment(on_review("First.")).unwrap();

    assert_eq!(review.close(at(9)), Ok(true));
    assert_eq!(review.close(at(10)), Ok(false));
    assert_eq!(review.state(), State::Closed);
    assert_eq!(review.closed_at, Some(at(9)));

    assert_eq!(review.add_comment(on_review("More.")), Err(Refusal::Closed));
    assert_eq!(
        review.reply(1, "Jane".to_owned(), at(10), "x".to_owned()),
        Err(Refusal::Closed)
    );
    assert_eq!(
        review.edit(1, Edit {
            body: Some("x".to_owned()),
            ..Edit::default()
        }),
        Err(Refusal::Closed)
    );
    assert_eq!(review.withdraw(1), Err(Refusal::Closed));
    assert_eq!(review.resolve(1, at(10)), Err(Refusal::Closed));
}

#[test]
fn a_reopened_review_takes_changes_again() {
    let mut review = review();
    review.close(at(9)).unwrap();

    assert!(review.reopen());
    assert_eq!(review.state(), State::Open);
    assert_eq!(review.closed_at, None);
    assert_eq!(review.add_comment(on_review("More.")), Ok(1));

    assert!(!review.reopen(), "it was open already");
}

#[test]
fn versions_count_up_and_the_base_is_found_by_its_label() {
    let mut review = Review::new("t".to_owned(), "Jane".to_owned(), at(0), vec![
        "a.md".to_owned(),
        "b.md".to_owned(),
    ]);
    assert_eq!(review.base(), None);

    let base = review.record(
        Files::from([
            ("a.md".to_owned(), Some("aaaa".to_owned())),
            ("b.md".to_owned(), None),
        ]),
        Some(BASE_LABEL),
        at(0),
    );
    let current = review.record(
        Files::from([
            ("a.md".to_owned(), Some("bbbb".to_owned())),
            ("b.md".to_owned(), Some("cccc".to_owned())),
        ]),
        None,
        at(0),
    );

    assert_eq!((base, current), (1, 2));
    assert_eq!(review.base(), Some(1));
    assert_eq!(review.latest().map(|version| version.number), Some(2));
    assert_eq!(review.blob(1, "a.md"), Some("aaaa"));
    assert_eq!(review.blob(1, "b.md"), None);
    assert_eq!(review.blob(3, "a.md"), None);
}

#[test]
fn refusals_name_the_comment_in_the_review() {
    assert_eq!(Refusal::Closed.message(id()), "R-1h4k2m9 is closed.");
    assert_eq!(
        refused(3, Why::Missing).message(id()),
        "No comment R-1h4k2m9#3."
    );
    assert_eq!(
        refused(3, Why::Withdrawn).message(id()),
        "R-1h4k2m9#3 was withdrawn."
    );
    assert_eq!(
        refused(3, Why::Draft).message(id()),
        "R-1h4k2m9#3 is a draft, not yet sent."
    );
    assert_eq!(
        refused(2, Why::DraftReply).message(id()),
        "R-1h4k2m9#2 holds a draft reply; withdraw it before resolving the comment."
    );
    assert_eq!(
        refused(2, Why::Answered).message(id()),
        "The assistant answered R-1h4k2m9#2 last; reply to it before sending it."
    );
    assert_eq!(
        refused(2, Why::VerdictRequired).message(id()),
        "A reply to R-1h4k2m9#2, the reviewer's comment, needs a verdict: accept, amend, decline, \
         defer, or ask."
    );
    assert_eq!(
        refused(2, Why::VerdictRefused).message(id()),
        "R-1h4k2m9#2 was opened by the assistant, so a reply to it takes no verdict."
    );
    assert_eq!(
        Refusal::NothingToSend.message(id()),
        "R-1h4k2m9 has nothing to send: no draft, and no comment waiting on the assistant. Pass \
         `-m` to send a message on its own."
    );
    assert_eq!(
        Refusal::Bound("jp-c17629384710".to_owned()).message(id()),
        "R-1h4k2m9 runs its passes in jp-c17629384710; leave out `--id` to continue there."
    );
    assert_eq!(
        Refusal::PassRunning(2).message(id()),
        "Pass 2 of R-1h4k2m9 has not ended."
    );
    assert_eq!(
        Refusal::Approval(3).message(id()),
        "R-1h4k2m9 has no approval 3."
    );
    assert_eq!(
        Refusal::ApprovalWithoutRange.message(id()),
        "An approval is of a range of a file: lines, or a span within them."
    );
}

/// `review.json` is read by every later `jp-review`, so its shape is pinned.
#[test]
fn a_review_is_stored_in_its_format() {
    let mut review = review();
    review
        .add_comment(on_range("2:1-2:4", Some("Two")))
        .unwrap();

    let json = serde_json::to_string_pretty(&review).unwrap();

    assert_eq!(
        json,
        r#"{
  "format": 1,
  "revision": 1,
  "title": "Review of a.md",
  "author": "Jane Doe <jane@example.com>",
  "created_at": "2026-11-17T10:00:00Z",
  "paths": [
    "a.md"
  ],
  "versions": [
    {
      "number": 1,
      "created_at": "2026-11-17T10:00:00Z",
      "files": {
        "a.md": "aaaa"
      }
    }
  ],
  "comments": [
    {
      "number": 1,
      "role": "reviewer",
      "author": "Jane Doe <jane@example.com>",
      "created_at": "2026-11-17T10:01:00Z",
      "anchor": {
        "version": 1,
        "path": "a.md",
        "range": "2:1-2:4"
      },
      "body": "",
      "suggestion": "Two"
    }
  ],
  "last_comment": 1
}"#
    );
    assert_eq!(serde_json::from_str::<Review>(&json).unwrap(), review);
}
