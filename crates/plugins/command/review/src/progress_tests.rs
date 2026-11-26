use chrono::TimeZone as _;
use pretty_assertions::assert_eq;

use super::*;
use crate::model::{Files, Review};

fn at(minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 11, 17, 10, minute, 0).unwrap()
}

/// A review of one file, with one version.
fn review() -> Review {
    let mut review = Review::new("Review of a.md".to_owned(), "Jane".to_owned(), at(0), vec![
        "a.md".to_owned(),
    ]);
    review.record(
        Files::from([("a.md".to_owned(), Some("aaaa".to_owned()))]),
        None,
        at(0),
    );

    review
}

fn lines(range: &str) -> Anchor {
    Anchor {
        version: 1,
        target: Target::Range("a.md".to_owned(), range.parse().unwrap()),
    }
}

fn until(line: usize) -> ReadUntil {
    ReadUntil {
        version: 1,
        line,
        author: "Jane".to_owned(),
        created_at: at(2),
    }
}

#[test]
fn approvals_are_numbered_in_the_order_made() {
    let mut progress = Progress::default();

    assert_eq!(
        progress.approve(lines("2-3"), "Jane".to_owned(), at(1)),
        Ok(1)
    );
    assert_eq!(
        progress.approve(lines("7"), "Jane".to_owned(), at(2)),
        Ok(2)
    );

    assert_eq!(progress.approvals, [
        Approval {
            number: 1,
            author: "Jane".to_owned(),
            created_at: at(1),
            anchor: lines("2-3"),
        },
        Approval {
            number: 2,
            author: "Jane".to_owned(),
            created_at: at(2),
            anchor: lines("7"),
        },
    ]);
}

#[test]
fn approving_a_range_twice_keeps_the_first_approval() {
    let mut progress = Progress::default();
    progress
        .approve(lines("2-3"), "Jane".to_owned(), at(1))
        .unwrap();

    assert_eq!(
        progress.approve(lines("2-3"), "Jane".to_owned(), at(5)),
        Ok(1)
    );
    assert_eq!(progress.approvals.len(), 1);
    assert_eq!(progress.approvals[0].created_at, at(1));
}

#[test]
fn only_a_range_can_be_approved() {
    let mut progress = Progress::default();
    let whole_file = Anchor {
        version: 1,
        target: Target::File("a.md".to_owned()),
    };
    let whole_review = Anchor {
        version: 1,
        target: Target::Review,
    };

    assert_eq!(
        progress.approve(whole_file, "Jane".to_owned(), at(1)),
        Err(Refusal::ApprovalWithoutRange)
    );
    assert_eq!(
        progress.approve(whole_review, "Jane".to_owned(), at(1)),
        Err(Refusal::ApprovalWithoutRange)
    );
    assert_eq!(progress.approvals, []);
}

#[test]
fn an_approval_is_taken_back_by_its_number() {
    let mut progress = Progress::default();
    progress
        .approve(lines("2-3"), "Jane".to_owned(), at(1))
        .unwrap();
    progress
        .approve(lines("7"), "Jane".to_owned(), at(2))
        .unwrap();

    assert_eq!(progress.unapprove(1), Ok(()));
    assert_eq!(progress.unapprove(1), Err(Refusal::Approval(1)));
    assert_eq!(progress.unapprove(9), Err(Refusal::Approval(9)));

    let left: Vec<u32> = progress
        .approvals
        .iter()
        .map(|approval| approval.number)
        .collect();
    assert_eq!(left, [2]);
}

#[test]
fn a_number_taken_back_is_not_given_to_the_next_approval() {
    let mut progress = Progress::default();
    progress
        .approve(lines("2-3"), "Jane".to_owned(), at(1))
        .unwrap();
    progress
        .approve(lines("7"), "Jane".to_owned(), at(2))
        .unwrap();
    progress.unapprove(2).unwrap();

    assert_eq!(
        progress.approve(lines("9"), "Jane".to_owned(), at(3)),
        Ok(3)
    );
    assert_eq!(progress.unapprove(2), Err(Refusal::Approval(2)));

    let left: Vec<u32> = progress
        .approvals
        .iter()
        .map(|approval| approval.number)
        .collect();
    assert_eq!(left, [1, 3]);
}

#[test]
fn approvals_stored_without_the_highest_number_are_numbered_on_from() {
    let mut progress: Progress = serde_json::from_str(
        r#"{"approvals": [{"number": 4, "author": "Jane", "created_at": "2026-11-17T10:01:00Z",
            "anchor": {"version": 1, "path": "a.md", "range": "2"}}]}"#,
    )
    .unwrap();

    assert_eq!(
        progress.approve(lines("9"), "Jane".to_owned(), at(3)),
        Ok(5)
    );
}

#[test]
fn a_file_is_read_to_one_line_at_a_time() {
    let mut progress = Progress::default();

    progress.read("a.md".to_owned(), until(40));
    progress.read("a.md".to_owned(), until(90));

    assert_eq!(
        progress.read_until,
        BTreeMap::from([("a.md".to_owned(), until(90))])
    );
}

#[test]
fn forgetting_how_far_a_file_was_read_says_whether_it_was_recorded() {
    let mut progress = Progress::default();
    progress.read("a.md".to_owned(), until(40));

    assert!(progress.unread("a.md"));
    assert!(!progress.unread("a.md"));
    assert_eq!(progress.read_until, BTreeMap::new());
}

/// `review.json` is read by every later `jp-review`, so its shape is pinned.
#[test]
fn progress_is_stored_beside_the_comments() {
    let mut review = review();
    review
        .progress
        .approve(lines("2-3"), "Jane".to_owned(), at(1))
        .unwrap();
    review.progress.read("a.md".to_owned(), until(40));

    let json = serde_json::to_string_pretty(&review).unwrap();

    assert_eq!(
        json,
        r#"{
  "format": 1,
  "revision": 1,
  "title": "Review of a.md",
  "author": "Jane",
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
  "last_comment": 0,
  "approvals": [
    {
      "number": 1,
      "author": "Jane",
      "created_at": "2026-11-17T10:01:00Z",
      "anchor": {
        "version": 1,
        "path": "a.md",
        "range": "2-3"
      }
    }
  ],
  "last_approval": 1,
  "read_until": {
    "a.md": {
      "version": 1,
      "line": 40,
      "author": "Jane",
      "created_at": "2026-11-17T10:02:00Z"
    }
  }
}"#
    );
    assert_eq!(serde_json::from_str::<Review>(&json).unwrap(), review);
}
