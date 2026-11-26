use chrono::TimeZone as _;
use pretty_assertions::assert_eq;

use super::*;
use crate::{
    anchor::{Anchor, Target},
    model::{Content, Files, NewComment, Reply, Verdict},
};

fn at(minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 17, 10, minute, 0).unwrap()
}

const CONVERSATION: &str = "jp-c17629384710";

/// A review of `a.md`, with one version and the reviewer's drafts `bodies`.
fn review(bodies: &[&str]) -> Review {
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
    for body in bodies {
        review.add_comment(comment(Role::Reviewer, body)).unwrap();
    }

    review
}

fn comment(role: Role, body: &str) -> NewComment {
    NewComment {
        role,
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

fn new_pass(only: &[u32], message: Option<&str>, conversation: Option<&str>) -> NewPass {
    NewPass {
        author: "Jane Doe <jane@example.com>".to_owned(),
        created_at: at(2),
        version: 1,
        only: only.to_vec(),
        message: message.map(ToOwned::to_owned),
        conversation: conversation.map(ToOwned::to_owned),
    }
}

fn ended(outcome: Outcome, version: Option<u32>) -> Ended {
    Ended {
        outcome,
        at: at(5),
        version,
        error: None,
        answer: None,
    }
}

fn answer(review: &mut Review, number: u32) {
    review
        .answer(number, Reply {
            role: Role::Assistant,
            author: "jp".to_owned(),
            created_at: at(3),
            body: "Done.".to_owned(),
            verdict: Some(Verdict::Accept),
            pass: None,
        })
        .unwrap();
}

#[test]
fn a_pass_sends_every_comment_waiting_on_the_assistant() {
    let mut review = review(&["One.", "Two.", "Three."]);

    assert_eq!(review.pending(&[]), Ok(vec![1, 2, 3]));
    assert_eq!(review.submit(new_pass(&[], None, None)), Ok(1));

    let pass = review.passes.get(1).unwrap();
    assert_eq!(pass.comments, [1, 2, 3]);
    assert_eq!(pass.conversation, None);
    assert_eq!(pass.ended, None);
    assert!(
        review
            .comments
            .iter()
            .all(|comment| comment.pass == Some(1))
    );
    assert_eq!(review.passes.running().map(|pass| pass.number), Some(1));
}

#[test]
fn only_narrows_a_pass_to_the_comments_it_names() {
    let mut review = review(&["One.", "Two.", "Three."]);

    assert_eq!(review.submit(new_pass(&[2, 1, 2], None, None)), Ok(1));

    assert_eq!(review.passes.get(1).unwrap().comments, [1, 2]);
    assert_eq!(review.comment(3).unwrap().pass, None);
}

#[test]
fn only_refuses_a_comment_with_nothing_for_the_assistant() {
    let mut review = review(&["One.", "Two."]);
    review.submit(new_pass(&[], None, None)).unwrap();
    review.passes.end(1, ended(Outcome::Completed, Some(1)));
    answer(&mut review, 1);
    review.comments[1].resolved_at = Some(at(4));

    assert_eq!(
        review.pending(&[1]),
        Err(Refusal::Comment(1, Why::Answered))
    );
    assert_eq!(
        review.pending(&[2]),
        Err(Refusal::Comment(2, Why::Resolved))
    );
    assert_eq!(review.pending(&[7]), Err(Refusal::Comment(7, Why::Missing)));
}

/// A stopped pass leaves a comment the assistant never answered, which the next
/// pass sends again; one the assistant answered waits on the reviewer.
#[test]
fn an_unanswered_comment_is_sent_again() {
    let mut review = review(&["One.", "Two."]);
    review.submit(new_pass(&[], None, None)).unwrap();
    answer(&mut review, 1);
    review.passes.end(1, ended(Outcome::Stopped, Some(1)));

    assert_eq!(review.pending(&[]), Ok(vec![2]));
}

#[test]
fn a_draft_reply_goes_out_with_its_pass() {
    let mut review = review(&["One."]);
    review.submit(new_pass(&[], None, None)).unwrap();
    answer(&mut review, 1);
    review.passes.end(1, ended(Outcome::Completed, Some(1)));
    review
        .reply(1, "Jane".to_owned(), at(6), "Not quite.".to_owned())
        .unwrap();

    assert_eq!(review.submit(new_pass(&[], None, None)), Ok(2));

    let comment = review.comment(1).unwrap();
    assert_eq!(comment.draft_reply, None);
    assert_eq!(comment.pass, Some(1));
    let sent = comment.replies.last().unwrap();
    assert_eq!(
        (sent.role, sent.body.as_str(), sent.pass),
        (Role::Reviewer, "Not quite.", Some(2))
    );
}

#[test]
fn a_pass_needs_something_to_send_or_a_message() {
    let mut review = review(&[]);

    assert_eq!(
        review.submit(new_pass(&[], None, None)),
        Err(Refusal::NothingToSend)
    );
    assert_eq!(review.submit(new_pass(&[], Some("Carry on."), None)), Ok(1));
    assert_eq!(
        review.passes.get(1).unwrap().message.as_deref(),
        Some("Carry on.")
    );
}

#[test]
fn one_pass_runs_at_a_time() {
    let mut review = review(&["One."]);
    review.submit(new_pass(&[], None, None)).unwrap();

    assert_eq!(
        review.submit(new_pass(&[], Some("Again."), None)),
        Err(Refusal::PassRunning(1))
    );
}

#[test]
fn an_abandoned_pass_is_interrupted() {
    let mut review = review(&["One."]);
    review.submit(new_pass(&[], None, None)).unwrap();

    assert_eq!(review.passes.interrupt(at(9)), [1]);
    assert_eq!(review.passes.interrupt(at(9)), Vec::<u32>::new());

    assert_eq!(
        review.passes.get(1).unwrap().ended,
        Some(Ended {
            outcome: Outcome::Interrupted,
            at: at(9),
            version: None,
            error: None,
            answer: None,
        })
    );
    assert_eq!(review.passes.running(), None);
}

/// A new conversation exists once the host says so, so the review runs its
/// passes there from then on.
#[test]
fn the_conversation_the_host_created_is_the_reviews() {
    let mut review = review(&["One."]);
    review.submit(new_pass(&[], None, None)).unwrap();

    review.passes.created(1, CONVERSATION);

    assert_eq!(review.passes.conversation.as_deref(), Some(CONVERSATION));
    assert_eq!(
        review.passes.get(1).unwrap().conversation.as_deref(),
        Some(CONVERSATION)
    );
}

/// A conversation named with `--id` might not exist, so it becomes the review's
/// only once a turn ran there.
#[test]
fn a_named_conversation_is_the_reviews_once_a_turn_ran_there() {
    let mut named = review(&["One."]);
    named
        .submit(new_pass(&[], None, Some(CONVERSATION)))
        .unwrap();
    assert_eq!(named.passes.conversation, None);
    assert!(named.passes.admits(CONVERSATION));

    named.passes.end(1, ended(Outcome::Completed, Some(1)));
    assert_eq!(named.passes.conversation.as_deref(), Some(CONVERSATION));

    let mut failed = review(&["One."]);
    failed.submit(new_pass(&[], None, Some("jp-c1"))).unwrap();
    failed.passes.end(1, ended(Outcome::Failed, Some(1)));
    assert_eq!(failed.passes.conversation, None);
    assert!(!failed.passes.admits("jp-c1"));
}

#[test]
fn a_review_keeps_to_its_conversation() {
    let mut review = review(&["One.", "Two."]);
    review.submit(new_pass(&[1], None, None)).unwrap();
    review.passes.created(1, CONVERSATION);
    review.passes.end(1, ended(Outcome::Completed, Some(1)));

    assert_eq!(
        review.submit(new_pass(&[], None, Some("jp-c1"))),
        Err(Refusal::Bound(CONVERSATION.to_owned()))
    );
    assert_eq!(
        review.submit(new_pass(&[], None, Some(CONVERSATION))),
        Ok(2)
    );
    assert_eq!(
        review.passes.get(2).unwrap().conversation.as_deref(),
        Some(CONVERSATION)
    );
}

#[test]
fn only_the_reviews_conversation_or_its_running_pass_is_admitted() {
    let mut review = review(&["One."]);
    assert!(!review.passes.admits(CONVERSATION));

    review.submit(new_pass(&[], None, None)).unwrap();
    review.passes.created(1, CONVERSATION);

    assert!(review.passes.admits(CONVERSATION));
    assert!(!review.passes.admits("jp-c1"));
    assert!(!review.passes.admits(""));
}

/// The files enter a conversation once: a later pass sends what changed since
/// the last one the assistant answered there was sent, not since it ended,
/// since the version recorded at the end may hold edits the assistant never
/// read.
#[test]
fn the_first_pass_in_a_conversation_carries_the_files() {
    let mut review = review(&["One.", "Two.", "Three."]);
    review.submit(new_pass(&[1], None, None)).unwrap();
    assert_eq!(review.passes.carry(1), Carry::Files);

    review.passes.created(1, CONVERSATION);
    review.passes.end(1, ended(Outcome::Completed, Some(3)));
    review.submit(new_pass(&[2], None, None)).unwrap();

    assert_eq!(review.passes.carry(2), Carry::Changes {
        pass: 1,
        version: 1
    });
}

/// A failed turn may never have shown the assistant the files, so they go
/// again.
#[test]
fn a_pass_after_only_failed_ones_carries_the_files_again() {
    let mut review = review(&["One."]);
    review.submit(new_pass(&[], None, None)).unwrap();
    review.passes.created(1, CONVERSATION);
    review.passes.end(1, ended(Outcome::Failed, Some(1)));
    review.submit(new_pass(&[], None, None)).unwrap();

    assert_eq!(review.passes.carry(2), Carry::Files);
}

/// A pass changed the files from the version it was sent against to the one it
/// recorded when it ended, which a pass that is running, or was interrupted,
/// doesn't have.
#[test]
fn the_span_of_a_pass_is_known_once_it_recorded_the_files() {
    let mut review = review(&["One.", "Two."]);
    assert_eq!(review.passes.span(PassRef::Last), Err(Unspanned::NoPasses));

    review.submit(new_pass(&[1], None, None)).unwrap();
    assert_eq!(
        review.passes.span(PassRef::Number(1)),
        Err(Unspanned::Running(1))
    );

    review.passes.end(1, ended(Outcome::Completed, Some(3)));
    review.submit(new_pass(&[2], None, None)).unwrap();
    review.passes.interrupt(at(9));

    assert_eq!(review.passes.span(PassRef::Number(1)), Ok((1, [1, 3])));
    assert_eq!(
        review.passes.span(PassRef::Last),
        Err(Unspanned::Unrecorded(2))
    );
    assert_eq!(
        review.passes.span(PassRef::Number(3)),
        Err(Unspanned::Missing(3))
    );
    assert_eq!(
        Unspanned::Unrecorded(2).message("R-1h4k2m9"),
        "Pass 2 of R-1h4k2m9 ended without recording the files, so what it changed isn't known."
    );
}

#[test]
fn a_pass_is_named_by_its_number_or_as_the_last() {
    assert_eq!(PassRef::parse(" 2 "), Ok(PassRef::Number(2)));
    assert_eq!(PassRef::parse("Last"), Ok(PassRef::Last));
    assert_eq!(
        PassRef::parse("-1"),
        Err("write a pass's number, such as `2`, or `last`".to_owned())
    );
}

#[test]
fn outcomes_are_written_in_lowercase() {
    let names: Vec<String> = [
        Outcome::Completed,
        Outcome::Stopped,
        Outcome::Failed,
        Outcome::Interrupted,
    ]
    .iter()
    .map(ToString::to_string)
    .collect();

    assert_eq!(names, ["completed", "stopped", "failed", "interrupted"]);
}

/// `review.json` is read by every later `jp-review`, so the shape of a pass is
/// pinned.
#[test]
fn a_pass_is_stored_in_its_format() {
    let mut review = review(&["One."]);
    review
        .submit(new_pass(&[], Some("First half only."), None))
        .unwrap();
    review.passes.created(1, CONVERSATION);
    review.passes.end(1, ended(Outcome::Completed, Some(2)));

    assert_eq!(
        serde_json::to_string_pretty(&review.passes).unwrap(),
        r#"{
  "conversation": "jp-c17629384710",
  "passes": [
    {
      "number": 1,
      "author": "Jane Doe <jane@example.com>",
      "created_at": "2026-11-17T10:02:00Z",
      "version": 1,
      "comments": [
        1
      ],
      "message": "First half only.",
      "conversation": "jp-c17629384710",
      "ended": {
        "outcome": "completed",
        "at": "2026-11-17T10:05:00Z",
        "version": 2
      }
    }
  ]
}"#
    );

    let json = serde_json::to_string(&review).unwrap();
    assert_eq!(serde_json::from_str::<Review>(&json).unwrap(), review);
}
