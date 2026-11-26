use chrono::{DateTime, TimeZone as _, Utc};
use pretty_assertions::assert_eq;

use super::*;
use crate::{
    anchor::{Anchor, Target},
    model::{Content, Files, NewComment, Reply, Verdict},
};

fn at(minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 17, 10, minute, 0).unwrap()
}

fn on_review(role: Role, body: &str) -> NewComment {
    NewComment {
        role,
        author: "Jane".to_owned(),
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

fn reply(verdict: Option<Verdict>, body: &str) -> Reply {
    Reply {
        role: Role::Assistant,
        author: "jp".to_owned(),
        created_at: at(2),
        body: body.to_owned(),
        verdict,
        pass: None,
    }
}

/// A review whose first pass is running, the reviewer's comment #1 sent in it.
fn running() -> Review {
    let mut review = Review::new("t".to_owned(), "Jane".to_owned(), at(0), vec![]);
    review.record(Files::new(), None, at(0));
    review
        .add_comment(on_review(Role::Reviewer, "One."))
        .unwrap();
    review
        .submit(NewPass {
            author: "Jane".to_owned(),
            created_at: at(1),
            version: 1,
            only: vec![],
            message: None,
            conversation: None,
        })
        .unwrap();

    review
}

#[test]
fn what_the_assistant_writes_during_the_pass_is_printed_once() {
    let mut review = running();
    let mut printed = Printed::default();
    assert_eq!(printed.new_lines(&review, 1), Vec::<String>::new());

    review
        .answer(
            1,
            reply(
                Some(Verdict::Accept),
                "\nFixed: it binds to loopback.\nMore.",
            ),
        )
        .unwrap();
    review
        .add_comment(on_review(Role::Assistant, "The examples disagree."))
        .unwrap();
    review
        .answer(2, reply(None, "Both now use R-1h4k2m9."))
        .unwrap();

    assert_eq!(printed.new_lines(&review, 1), [
        "#1  accept   Fixed: it binds to loopback.\n",
        "#2  opened   the review: The examples disagree.\n",
        "#2  reply    Both now use R-1h4k2m9.\n",
    ]);
    assert_eq!(printed.new_lines(&review, 1), Vec::<String>::new());
}

/// What the assistant wrote during another pass, or outside one, isn't this
/// pass's to print.
#[test]
fn what_the_assistant_wrote_outside_the_pass_is_not_printed() {
    let mut review = running();
    review.passes.end(1, Ended {
        outcome: Outcome::Completed,
        at: at(3),
        version: Some(1),
        error: None,
        answer: None,
    });
    review
        .answer(1, reply(Some(Verdict::Decline), "No."))
        .unwrap();

    assert_eq!(
        Printed::default().new_lines(&review, 1),
        Vec::<String>::new()
    );
}

/// The host's answer to a pass's query, the turn ending as `outcome`.
fn done(outcome: QueryOutcome, answer: Option<&str>) -> QueryCompleteResponse {
    QueryCompleteResponse {
        id: Some("pass".to_owned()),
        conversation: "jp-c17629384710".to_owned(),
        outcome,
        data: None,
        answer: answer.map(ToOwned::to_owned),
    }
}

#[test]
fn the_hosts_outcome_says_how_the_pass_ended() {
    assert_eq!(
        Ending::of(done(QueryOutcome::Finished, None)).outcome,
        Outcome::Completed
    );
    assert_eq!(
        Ending::of(done(QueryOutcome::Stopped, None)).outcome,
        Outcome::Stopped
    );
    assert_eq!(
        Ending::of(done(QueryOutcome::Aborted, None)).outcome,
        Outcome::Stopped
    );
    assert_eq!(Ending::of(done(QueryOutcome::Finished, None)).error, None);
}

#[test]
fn the_pass_keeps_the_closing_message_the_host_reports() {
    assert_eq!(
        Ending::of(done(QueryOutcome::Stopped, Some("Fixed #1; #2 next."))),
        Ending {
            outcome: Outcome::Stopped,
            error: None,
            answer: Some("Fixed #1; #2 next.".to_owned()),
        }
    );
}
