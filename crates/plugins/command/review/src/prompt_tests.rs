use std::collections::HashMap;

use chrono::{DateTime, TimeZone as _, Utc};
use pretty_assertions::assert_eq;

use super::*;
use crate::{
    anchor::{Anchor, Target},
    model::{Content, Files, NewComment, Reply, Verdict},
    pass::{Ended, NewPass, Outcome},
    progress::ReadUntil,
    text::Range,
};

const FIRST: &str = "# Title\n\nFirst line.\nSecond line.\n";
const SECOND: &str = "# Title\n\nFirst line.\nSecond line, longer.\n";

fn at(minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 17, 10, minute, 0).unwrap()
}

fn id() -> ReviewId {
    "R-1h4k2m9".parse().unwrap()
}

fn texts() -> Texts {
    HashMap::from([
        ("first".to_owned(), FIRST.to_owned()),
        ("second".to_owned(), SECOND.to_owned()),
    ])
}

fn version(blob: &str) -> Files {
    Files::from([
        ("docs/a.md".to_owned(), Some(blob.to_owned())),
        ("docs/b.md".to_owned(), None),
    ])
}

fn on(
    role: Role,
    version: u32,
    target: Target,
    body: &str,
    suggestion: Option<&str>,
) -> NewComment {
    NewComment {
        role,
        author: "Jane Doe <jane@example.com>".to_owned(),
        created_at: at(1),
        content: Content {
            anchor: Anchor { version, target },
            body: body.to_owned(),
            suggestion: suggestion.map(ToOwned::to_owned),
        },
    }
}

fn range(path: &str, range: &str) -> Target {
    Target::Range(path.to_owned(), range.parse::<Range>().unwrap())
}

fn new_pass(version: u32, message: Option<&str>) -> NewPass {
    NewPass {
        author: "Jane Doe <jane@example.com>".to_owned(),
        created_at: at(2),
        version,
        only: vec![],
        message: message.map(ToOwned::to_owned),
        conversation: None,
    }
}

fn reply(role: Role, verdict: Option<Verdict>, body: &str) -> Reply {
    Reply {
        role,
        author: "jp".to_owned(),
        created_at: at(3),
        body: body.to_owned(),
        verdict,
        pass: None,
    }
}

/// A review of `docs/a.md` and the absent `docs/b.md`, with its first pass
/// sent: a comment on a range, a suggestion, and a comment on the review.
fn first_pass() -> Review {
    let mut review = Review::new(
        "Review".to_owned(),
        "Jane Doe <jane@example.com>".to_owned(),
        at(0),
        vec!["docs/a.md".to_owned(), "docs/b.md".to_owned()],
    );
    review.record(version("first"), None, at(0));
    for new in [
        on(
            Role::Reviewer,
            1,
            range("docs/a.md", "3:1-3:6"),
            "Name it.",
            None,
        ),
        on(
            Role::Reviewer,
            1,
            range("docs/a.md", "4"),
            "",
            Some("Second."),
        ),
        on(Role::Reviewer, 1, Target::Review, "Too long.", None),
    ] {
        review.add_comment(new).unwrap();
    }
    review
        .submit(new_pass(1, Some("First half only.")))
        .unwrap();

    review
}

/// The first pass answered, the file edited, and a second pass sent: a reply to
/// a comment the assistant answered, the suggestion the assistant left
/// unanswered, a new comment, and a reply to a comment the assistant opened.
fn second_pass() -> Review {
    let mut review = first_pass();
    review.passes.created(1, "jp-c17629384710");
    review
        .add_comment(on(
            Role::Assistant,
            1,
            Target::File("docs/a.md".to_owned()),
            "The title is vague.",
            None,
        ))
        .unwrap();
    review
        .answer(
            1,
            reply(Role::Assistant, Some(Verdict::Ask), "Name it how?"),
        )
        .unwrap();
    review
        .answer(
            3,
            reply(Role::Assistant, Some(Verdict::Decline), "It's fine."),
        )
        .unwrap();
    review.passes.end(1, Ended {
        outcome: Outcome::Completed,
        at: at(4),
        version: Some(1),
        error: None,
        answer: None,
    });

    review.record(version("second"), None, at(5));
    review
        .reply(1, "Jane".to_owned(), at(5), "After the heading.".to_owned())
        .unwrap();
    review
        .reply(4, "Jane".to_owned(), at(5), "It names the RFD.".to_owned())
        .unwrap();
    review
        .add_comment(on(
            Role::Reviewer,
            2,
            range("docs/a.md", "4"),
            "Shorter.",
            None,
        ))
        .unwrap();
    review.submit(new_pass(2, None)).unwrap();

    review
}

fn message_of(review: &Review, number: u32) -> String {
    let pass = review.passes.get(number).unwrap();

    message(&Prompt {
        id: id(),
        review,
        pass,
        carry: review.passes.carry(number),
        texts: &texts(),
    })
}

#[test]
fn the_first_pass_carries_the_files_and_the_comments() {
    let review = first_pass();

    assert_eq!(
        message_of(&review, 1),
        "# Review R-1h4k2m9, pass 1

First half only.

## docs/a.md (version 1)

```
1 | # Title
2 |
3 | First line.
4 | Second line.
```

## docs/b.md (absent from version 1)

## Comments

### R-1h4k2m9#1 at docs/a.md:3:1-3:6

> First

Name it.

### R-1h4k2m9#2 at docs/a.md:4, suggestion

```diff
-Second line.
+Second.
```

### R-1h4k2m9#3 on the review

Too long.
"
    );
}

#[test]
fn a_later_pass_carries_what_changed_and_what_the_assistant_has_not_answered() {
    let review = second_pass();
    assert_eq!(review.passes.get(2).unwrap().comments, [1, 2, 4, 5]);

    assert_eq!(
        message_of(&review, 2),
        "# Review R-1h4k2m9, pass 2

## Changes since pass 1 was sent

This includes your own edits from that pass.

```diff
--- v1/docs/a.md
+++ v2/docs/a.md
@@ -1,4 +1,4 @@
 # Title\n \n First line.
-Second line.
+Second line, longer.
```

## Comments

### R-1h4k2m9#1 at docs/a.md:3:1-3:6

> First

#### Reply

After the heading.

### R-1h4k2m9#2 at docs/a.md:4 in version 1, outdated since version 2, suggestion

```diff
-Second line.
+Second.
```

### R-1h4k2m9#4 on docs/a.md, opened by the assistant

#### Reply

It names the RFD.

### R-1h4k2m9#5 at docs/a.md:4

> Second line, longer.

Shorter.
"
    );
}

/// Nothing changed since the last pass, and the pass sends only a message.
#[test]
fn a_pass_with_only_a_message_carries_only_the_message() {
    let mut review = first_pass();
    review.passes.created(1, "jp-c17629384710");
    for number in [1, 2, 3] {
        review
            .answer(
                number,
                reply(Role::Assistant, Some(Verdict::Accept), "Done."),
            )
            .unwrap();
    }
    review.passes.end(1, Ended {
        outcome: Outcome::Stopped,
        at: at(4),
        version: Some(1),
        error: None,
        answer: None,
    });
    review.submit(new_pass(1, Some("Carry on."))).unwrap();

    assert_eq!(
        message_of(&review, 2),
        "# Review R-1h4k2m9, pass 2\n\nCarry on.\n"
    );
}

/// A review of `docs/a.md` whose reviewer approved lines 3 and 4 and read
/// through line 3 in version 1, before version 2 changed line 4.
fn read_and_approved() -> Review {
    let mut review = Review::new(
        "Review".to_owned(),
        "Jane Doe <jane@example.com>".to_owned(),
        at(0),
        vec!["docs/a.md".to_owned(), "docs/b.md".to_owned()],
    );
    review.record(version("first"), None, at(0));
    for lines in ["3", "4"] {
        let anchor = Anchor {
            version: 1,
            target: range("docs/a.md", lines),
        };
        review
            .progress
            .approve(anchor, "Jane".to_owned(), at(1))
            .unwrap();
    }
    review.progress.read("docs/a.md".to_owned(), ReadUntil {
        version: 1,
        line: 3,
        author: "Jane".to_owned(),
        created_at: at(1),
    });
    review.record(version("second"), None, at(2));

    review
}

/// The approval of line 4 was of the text as it stood, which version 2 changed.
#[test]
fn a_pass_says_how_far_the_reviewer_read_and_what_they_approved() {
    let mut review = read_and_approved();
    review.submit(new_pass(2, Some("Go on."))).unwrap();

    assert_eq!(
        message_of(&review, 1),
        "# Review R-1h4k2m9, pass 1

Go on.

## docs/a.md (version 2)

```
1 | # Title
2 |
3 | First line.
4 | Second line, longer.
```

## docs/b.md (absent from version 2)

## Read so far

The reviewer has read these files this far, and no further. The lines after are unread: that they \
         carry no comment says nothing about them, and comments on them may follow in a later \
         pass.

- docs/a.md: through line 3

## Approved

The reviewer read these passages and approved them as they stand. Leave them as they are, unless \
         acting on a comment needs one changed; say so in your reply when it does.

- docs/a.md:3, from \"First line.\"
"
    );
}

#[test]
fn the_blobs_read_place_what_was_approved_and_read() {
    let mut review = read_and_approved();
    review.submit(new_pass(2, Some("Go on."))).unwrap();

    let read = blobs(&review, review.passes.get(1).unwrap(), Carry::Files);

    assert_eq!(read.into_iter().collect::<Vec<_>>(), ["first", "second"]);
}

#[test]
fn line_numbers_are_as_wide_as_the_last() {
    let text = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n";

    let numbered = numbered(text);

    assert!(numbered.starts_with(" 1 | 1\n 2 | 2\n"), "{numbered}");
    assert!(numbered.ends_with(" 9 | 9\n10 | 10\n"), "{numbered}");
}

#[test]
fn the_blobs_read_are_the_files_and_what_the_comments_quote() {
    let earlier = first_pass();
    let review = second_pass();

    let first = blobs(&earlier, earlier.passes.get(1).unwrap(), Carry::Files);
    let second = blobs(
        &review,
        review.passes.get(2).unwrap(),
        review.passes.carry(2),
    );

    assert_eq!(first.into_iter().collect::<Vec<_>>(), ["first"]);
    assert_eq!(second.into_iter().collect::<Vec<_>>(), ["first", "second"]);
}
