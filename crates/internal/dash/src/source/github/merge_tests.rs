use std::collections::HashSet;

use camino::Utf8PathBuf;
use camino_tempfile::Utf8TempDir;

use super::*;
use crate::{
    model::{Checks, StackPlace},
    source::github::label::DEFAULT_QUEUE_LABEL,
};

fn github() -> GitHub {
    GitHub {
        dir: Utf8PathBuf::from("."),
        label: DEFAULT_QUEUE_LABEL.to_owned(),
        file: None,
        state: super::super::State::default(),
        dirty: false,
        requested: HashSet::new(),
        default_branch: Some("main".to_owned()),
        prefetched: HashMap::new(),
        stacks: true,
        driver: Driver::default(),
    }
}

fn pr(number: u64, sha: &str) -> PullRequest {
    PullRequest {
        number,
        title: format!("PR {number}"),
        url: String::new(),
        head: format!("branch-{number}"),
        sha: sha.to_owned(),
        draft: false,
        checks: Checks::default(),
        threads: 0,
        answered: 0,
        labeled: true,
        base: None,
        stack: None,
        remote_branch: None,
        mergeable: None,
        queue: None,
    }
}

fn blocked(sha: &str) -> Hold {
    Hold {
        sha: sha.to_owned(),
        reason: Reason::Blocked("merging it failed: HTTP 405: no".to_owned()),
    }
}

#[test]
fn test_update_responses() {
    assert_eq!(
        classify_update(202, r#"{"message": "Updating"}"#),
        Sent::Done
    );
    assert_eq!(
        classify_update(
            422,
            r#"{"message": "expected head sha didn't match current head ref."}"#
        ),
        Sent::Moved
    );
    assert_eq!(
        classify_update(
            422,
            r#"{"message": "merge conflict between base and head"}"#
        ),
        Sent::Conflicts
    );
    assert_eq!(
        classify_update(403, r#"{"message": "Resource not accessible"}"#),
        Sent::Refused("updating it failed: HTTP 403: Resource not accessible".to_owned())
    );
}

#[test]
fn test_merge_responses() {
    assert_eq!(classify_merge(200, r#"{"merged": true}"#), Sent::Done);
    assert_eq!(
        classify_merge(409, r#"{"message": "Head branch was modified."}"#),
        Sent::Moved
    );
    assert_eq!(
        classify_merge(
            405,
            r#"{"message": "Base branch was modified. Review and try the merge again."}"#
        ),
        Sent::Moved
    );
    assert_eq!(
        classify_merge(
            405,
            r#"{"message": "Required status check \"lint\" is expected."}"#
        ),
        Sent::Refused(
            "merging it failed: HTTP 405: Required status check \"lint\" is expected.".to_owned()
        )
    );
}

#[test]
fn test_merge_method_prefers_squash_then_merge_then_rebase() {
    let allow = |squash, merge, rebase| Settings {
        allow_squash_merge: squash,
        allow_merge_commit: merge,
        allow_rebase_merge: rebase,
        ..Settings::default()
    };

    assert_eq!(choose_method(Some(&allow(true, true, true))), "squash");
    assert_eq!(choose_method(Some(&allow(false, true, true))), "merge");
    assert_eq!(choose_method(Some(&allow(false, false, true))), "rebase");
    // Unknown (a viewer who can't see the settings) falls back to squash.
    assert_eq!(choose_method(Some(&allow(false, false, false))), "squash");
    assert_eq!(choose_method(None), "squash");
}

#[test]
fn test_slim_keeps_only_the_fields_read() {
    let compare = r#"{"status": "behind", "behind_by": 3, "ahead_by": 1,
                      "files": [{"filename": "a.rs", "patch": "@@ -1 +1 @@"}]}"#;

    assert_eq!(slim::<Compare>(compare).unwrap(), r#"{"behind_by":3}"#);
    assert_eq!(
        slim::<super::super::Detail>(r#"{"number": 7, "mergeable": null}"#).unwrap(),
        r#"{"mergeable":null}"#
    );
}

#[test]
fn test_only_one_driver_holds_the_queue_lock() {
    let dir = Utf8TempDir::new().unwrap();
    let path = dir.path().join("jp/dash/repo.queue.lock");

    let mut first = Driver::default();
    let mut second = Driver::default();
    assert!(first.acquire_at(&path));
    assert!(!second.acquire_at(&path));

    // Releasing the lock lets the second driver acquire it.
    drop(first);

    // A child process spawned by a test running in parallel can hold a copy of
    // `first`'s descriptor until it execs and close-on-exec closes it, which
    // keeps the lock held for a few milliseconds after the drop. Drivers retry
    // the lock on every poll; the test retries too, up to a deadline.
    let deadline = Instant::now() + Duration::from_secs(5);
    while !second.acquire_at(&path) {
        assert!(
            Instant::now() < deadline,
            "lock still held 5s after the first driver released it"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn test_events_are_reported_once() {
    let mut driver = Driver::default();
    let mut poll = Poll::default();

    driver.report("7:abc:hold".to_owned(), "held".to_owned(), true, &mut poll);
    driver.report("7:abc:hold".to_owned(), "held".to_owned(), true, &mut poll);
    driver.report(
        "7:def:hold".to_owned(),
        "held again".to_owned(),
        true,
        &mut poll,
    );

    assert_eq!(poll.events, [
        Event {
            message: "held".to_owned(),
            notify: true,
        },
        Event {
            message: "held again".to_owned(),
            notify: true,
        },
    ]);
}

#[test]
fn test_cleared_numbers_are_taken_once() {
    let control = QueueControl::default();
    control.clear(7);
    control.clear(9);

    assert_eq!(control.take_cleared(), [7, 9]);
    assert_eq!(control.take_cleared(), Vec::<u64>::new());
}

#[test]
fn test_a_hold_lasts_until_the_head_commit_changes() {
    let mut github = github();
    let mut driver = Driver {
        holds: HashMap::from([(1, blocked("old")), (2, blocked("same"))]),
        ..Driver::default()
    };
    let mut prs = vec![pr(1, "new"), pr(2, "same")];
    let mut poll = Poll::default();

    // Paused, so nothing reaches GitHub.
    let role = driver.drive(&mut github, true, &mut prs, &mut poll);

    assert_eq!(role, QueueRole::Paused);
    assert_eq!(driver.holds.keys().collect::<Vec<_>>(), [&2]);
    assert_eq!(prs[0].queue, Some(QueueState::Waiting { position: 1 }));
    assert_eq!(
        prs[1].queue,
        Some(QueueState::Blocked(
            "merging it failed: HTTP 405: no".to_owned()
        ))
    );
    assert_eq!(poll.events, [] as [Event; 0]);
}

#[test]
fn test_a_hold_ends_when_the_label_comes_off() {
    let mut github = github();
    let mut driver = Driver {
        holds: HashMap::from([(1, blocked("sha"))]),
        ..Driver::default()
    };
    let mut prs = vec![PullRequest {
        labeled: false,
        ..pr(1, "sha")
    }];

    driver.drive(&mut github, true, &mut prs, &mut Poll::default());

    assert!(driver.holds.is_empty());
}

#[test]
fn test_forget_lifts_a_hold_at_the_next_drive() {
    let mut github = github();
    let mut driver = Driver {
        holds: HashMap::from([(1, blocked("sha")), (2, blocked("sha"))]),
        ..Driver::default()
    };
    let mut prs = vec![pr(1, "sha"), pr(2, "sha")];

    driver.forget(&[1]);
    driver.drive(&mut github, true, &mut prs, &mut Poll::default());

    assert_eq!(driver.holds.keys().collect::<Vec<_>>(), [&2]);
    assert_eq!(prs[0].queue, Some(QueueState::Waiting { position: 1 }));
}

#[test]
fn test_forgetting_any_member_lifts_a_stacks_hold() {
    let mut github = github();
    let stacked = |number, position, labeled| PullRequest {
        labeled,
        stack: Some(StackPlace {
            stack: 1,
            position,
            size: 2,
            branch: format!("branch-{number}"),
            below: None,
        }),
        ..pr(number, &format!("sha-{number}"))
    };
    let mut driver = Driver {
        holds: HashMap::from([(1, blocked("sha-1 sha-2"))]),
        ..Driver::default()
    };
    let mut prs = vec![stacked(1, 1, false), stacked(2, 2, true)];

    // The top was put back in the queue; the hold is keyed by the bottom.
    driver.forget(&[2]);
    driver.drive(&mut github, true, &mut prs, &mut Poll::default());

    assert!(driver.holds.is_empty());
}

fn merged(number: u64, branch: &str, stack: Option<(u64, usize)>) -> Merged {
    Merged {
        number,
        branch: branch.to_owned(),
        stack,
    }
}

#[test]
fn test_merged_branches_are_deleted() {
    let prs = vec![pr(1, "a"), pr(2, "b")];

    assert_eq!(deletable(&[merged(1, "feature", None)], &prs, "main"), [(
        1,
        "feature".to_owned()
    )]);
}

#[test]
fn test_a_branch_another_pull_request_builds_on_stays() {
    // #2 targets #1's branch, outside any stack.
    let prs = vec![pr(1, "a"), PullRequest {
        base: Some("feature".to_owned()),
        ..pr(2, "b")
    }];

    assert_eq!(deletable(&[merged(1, "feature", None)], &prs, "main"), []
        as [(u64, String); 0]);
}

#[test]
fn test_a_stacks_top_branch_stays_while_the_stack_goes_on() {
    let place = |position| StackPlace {
        stack: 4,
        position,
        size: 3,
        branch: String::new(),
        below: None,
    };
    // #1 and #2 merged; #3 sits on #2 and stays open.
    let prs = vec![
        PullRequest {
            stack: Some(place(1)),
            ..pr(1, "a")
        },
        PullRequest {
            stack: Some(place(2)),
            ..pr(2, "b")
        },
        PullRequest {
            stack: Some(place(3)),
            ..pr(3, "c")
        },
    ];
    let done = [merged(1, "b1", Some((4, 1))), merged(2, "b2", Some((4, 2)))];

    assert_eq!(deletable(&done, &prs, "main"), [(1, "b1".to_owned())]);
}

#[test]
fn test_the_default_branch_is_never_deleted() {
    assert_eq!(
        deletable(&[merged(1, "main", None)], &[pr(1, "a")], "main"),
        [] as [(u64, String); 0]
    );
}

#[test]
fn test_a_merge_records_only_branches_in_the_repository() {
    let mut driver = Driver::default();
    let own = PullRequest {
        remote_branch: Some("feature".to_owned()),
        ..pr(1, "a")
    };
    let fork = pr(2, "b");

    driver.merged(
        &Entry {
            members: vec![&own],
        },
        &mut Poll::default(),
    );
    driver.merged(
        &Entry {
            members: vec![&fork],
        },
        &mut Poll::default(),
    );

    assert_eq!(driver.merged, [merged(1, "feature", None)]);
}

#[test]
fn test_async_merge_answers() {
    assert_eq!(
        classify_async(200, r#"{"status": "merged", "details": {}}"#),
        Async::Done
    );
    assert_eq!(
        classify_async(202, r#"{"status": "enqueued"}"#),
        Async::Done
    );
    assert_eq!(
        classify_async(
            202,
            r#"{"status": "pending", "details": {"uuid": "6b1f-42"}}"#
        ),
        Async::Pending("6b1f-42".to_owned())
    );
    assert_eq!(
        classify_async(
            200,
            r#"{"status": "failed", "details": {"message": "Stack is not linear"}}"#
        ),
        Async::Refused("merging it failed: Stack is not linear".to_owned())
    );
    assert_eq!(
        classify_async(409, r#"{"message": "Head branch was modified."}"#),
        Async::Moved
    );
    assert_eq!(
        classify_async(422, r#"{"message": "Validation Failed"}"#),
        Async::Refused("merging it failed: HTTP 422: Validation Failed".to_owned())
    );
}

#[test]
fn test_a_pending_async_merge_without_a_request_is_refused() {
    assert_eq!(
        classify_async(202, r#"{"status": "pending"}"#),
        Async::Refused("merging it failed: GitHub named no request to follow".to_owned())
    );
}
