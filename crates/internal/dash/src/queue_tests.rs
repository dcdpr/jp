use super::*;
use crate::model::{Checks, StackPlace};

fn stacked(number: u64, position: usize, size: usize, labeled: bool) -> PullRequest {
    PullRequest {
        labeled,
        stack: Some(StackPlace {
            stack: 1,
            position,
            size,
            branch: format!("branch-{number}"),
            below: None,
        }),
        ..pr(number)
    }
}

fn pr(number: u64) -> PullRequest {
    PullRequest {
        number,
        title: format!("PR {number}"),
        url: format!("https://github.com/owner/repo/pull/{number}"),
        head: format!("branch-{number}"),
        sha: format!("sha-{number}"),
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

fn states(prs: &[PullRequest]) -> Vec<(u64, Option<QueueState>)> {
    prs.iter().map(|pr| (pr.number, pr.queue.clone())).collect()
}

fn waiting_at(position: usize) -> QueueState {
    QueueState::Waiting { position }
}

const PASSED: Checks = Checks {
    passed: 3,
    pending: 0,
    failed: 0,
};

#[test]
fn test_labeled_pull_requests_line_up_by_number() {
    let mut prs = vec![pr(12), pr(3), pr(7)];

    assign(&mut prs, None, &HashMap::new());

    assert_eq!(states(&prs), [
        (3, Some(waiting_at(1))),
        (7, Some(waiting_at(2))),
        (12, Some(waiting_at(3))),
    ]);
    assert_eq!(waiting(&prs), 3);
    assert_eq!(front(&prs).map(|entry| entry.key()), Some(3));
}

#[test]
fn test_the_front_keeps_its_place_when_a_lower_number_joins() {
    let mut prs = vec![pr(3), pr(7)];

    assign(&mut prs, Some(7), &HashMap::new());

    assert_eq!(states(&prs), [
        (3, Some(waiting_at(2))),
        (7, Some(waiting_at(1)))
    ]);
    assert_eq!(front(&prs).map(|entry| entry.key()), Some(7));
}

#[test]
fn test_unlabeled_pull_requests_are_not_in_the_queue() {
    let mut prs = vec![pr(1), PullRequest {
        labeled: false,
        ..pr(2)
    }];

    assign(&mut prs, None, &HashMap::new());

    assert_eq!(states(&prs), [(1, Some(waiting_at(1))), (2, None)]);
}

#[test]
fn test_held_back_pull_requests_give_up_their_place() {
    let failing = Checks {
        passed: 3,
        pending: 0,
        failed: 1,
    };
    let mut prs = vec![
        PullRequest {
            draft: true,
            ..pr(1)
        },
        PullRequest {
            checks: failing,
            ..pr(2)
        },
        PullRequest {
            base: Some("feature".to_owned()),
            ..pr(3)
        },
        pr(4),
        pr(5),
        pr(6),
    ];
    let holds = HashMap::from([
        (4, Hold {
            sha: "sha-4".to_owned(),
            reason: Reason::Conflicts,
        }),
        (5, Hold {
            sha: "sha-5".to_owned(),
            reason: Reason::Blocked("merging it failed".to_owned()),
        }),
    ]);

    assign(&mut prs, None, &holds);

    assert_eq!(states(&prs), [
        (1, Some(QueueState::Draft)),
        (2, Some(QueueState::Failing)),
        (3, Some(QueueState::OtherBase("feature".to_owned()))),
        (4, Some(QueueState::Conflicts)),
        (5, Some(QueueState::Blocked("merging it failed".to_owned()))),
        (6, Some(waiting_at(1))),
    ]);
    assert_eq!(waiting(&prs), 1);
}

#[test]
fn test_a_hold_on_an_older_commit_does_not_apply() {
    let mut prs = vec![pr(4)];
    let holds = HashMap::from([(4, Hold {
        sha: "pushed-over".to_owned(),
        reason: Reason::Conflicts,
    })]);

    assign(&mut prs, None, &holds);

    assert_eq!(states(&prs), [(4, Some(waiting_at(1)))]);
}

#[test]
fn test_pending_checks_keep_a_pull_request_in_line() {
    let pending = Checks {
        passed: 3,
        pending: 2,
        failed: 0,
    };
    let mut prs = vec![PullRequest {
        checks: pending,
        ..pr(1)
    }];

    assign(&mut prs, None, &HashMap::new());

    assert_eq!(states(&prs), [(1, Some(waiting_at(1)))]);
}

#[test]
fn test_a_stack_entry_runs_from_its_bottom_to_its_highest_labeled_pull_request() {
    // Stack, bottom to top: #20, #12, #31, #40. #31 is the highest labeled
    // one, so #40 stays out.
    let prs = vec![
        stacked(31, 3, 4, true),
        stacked(20, 1, 4, false),
        stacked(40, 4, 4, false),
        stacked(12, 2, 4, true),
    ];

    let entries = entries(&prs);

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].numbers(), [20, 12, 31]);
    assert_eq!(entries[0].key(), 20);
    assert_eq!(entries[0].top().number, 31);
    assert_eq!(entries[0].name(), "#20..#31");
    assert_eq!(entries[0].signature(), "sha-20 sha-12 sha-31");
    assert!(entries[0].stacked());
}

#[test]
fn test_a_member_github_cannot_merge_shows_conflicts_anywhere_in_line() {
    let mut prs = vec![pr(1), stacked(5, 1, 2, false), PullRequest {
        mergeable: Some(false),
        ..stacked(6, 2, 2, true)
    }];

    assign(&mut prs, None, &HashMap::new());

    assert_eq!(states(&prs), [
        (1, Some(waiting_at(1))),
        (5, Some(QueueState::Conflicts)),
        (6, Some(QueueState::Conflicts)),
    ]);
}

#[test]
fn test_a_stack_without_a_label_is_not_in_the_queue() {
    let prs = vec![stacked(1, 1, 2, false), stacked(2, 2, 2, false)];

    assert_eq!(entries(&prs), [] as [Entry<'_>; 0]);
}

#[test]
fn test_every_member_shares_its_stacks_place_in_line() {
    let mut prs = vec![pr(5), stacked(9, 1, 2, false), stacked(3, 2, 2, true)];

    assign(&mut prs, None, &HashMap::new());

    // The stack's bottom is #9, so it lines up after #5 even though #3 is in
    // it.
    assert_eq!(states(&prs), [
        (3, Some(waiting_at(2))),
        (5, Some(waiting_at(1))),
        (9, Some(waiting_at(2))),
    ]);
    assert_eq!(waiting(&prs), 2);
}

#[test]
fn test_one_failing_member_holds_back_the_whole_stack() {
    let failing = Checks {
        passed: 1,
        pending: 0,
        failed: 1,
    };
    let mut prs = vec![
        PullRequest {
            checks: failing,
            ..stacked(1, 1, 2, false)
        },
        stacked(2, 2, 2, true),
    ];

    assign(&mut prs, None, &HashMap::new());

    assert_eq!(states(&prs), [
        (1, Some(QueueState::Failing)),
        (2, Some(QueueState::Failing)),
    ]);
}

#[test]
fn test_a_stack_hold_lifts_when_any_member_gets_a_new_commit() {
    let holds = HashMap::from([(1, Hold {
        sha: "sha-1 sha-2".to_owned(),
        reason: Reason::Conflicts,
    })]);

    let mut held = vec![stacked(1, 1, 2, false), stacked(2, 2, 2, true)];
    assign(&mut held, None, &holds);
    assert_eq!(states(&held), [
        (1, Some(QueueState::Conflicts)),
        (2, Some(QueueState::Conflicts)),
    ]);

    let mut pushed = vec![stacked(1, 1, 2, false), PullRequest {
        sha: "pushed".to_owned(),
        ..stacked(2, 2, 2, true)
    }];
    assign(&mut pushed, None, &holds);
    assert_eq!(states(&pushed), [
        (1, Some(waiting_at(1))),
        (2, Some(waiting_at(1))),
    ]);
}

fn status(checks: Checks, behind: u64, mergeable: Option<bool>) -> Status {
    Status {
        checks,
        behind,
        mergeable,
        quiet: false,
    }
}

#[test]
fn test_next_merges_a_green_up_to_date_mergeable_front() {
    assert_eq!(next(&status(PASSED, 0, Some(true))), Next::Merge);
}

#[test]
fn test_next_updates_a_front_behind_its_base_before_waiting_for_checks() {
    let pending = Checks {
        pending: 2,
        ..PASSED
    };

    assert_eq!(next(&status(pending, 3, Some(true))), Next::Update);
    assert_eq!(next(&status(PASSED, 1, None)), Next::Update);
}

#[test]
fn test_next_holds_a_front_that_conflicts() {
    assert_eq!(next(&status(PASSED, 4, Some(false))), Next::Conflicts);
}

#[test]
fn test_next_waits_for_checks_then_for_github() {
    let pending = Checks {
        pending: 1,
        ..PASSED
    };

    assert_eq!(
        next(&status(pending, 0, Some(true))),
        Next::Wait(Step::WaitingForChecks)
    );
    assert_eq!(next(&status(PASSED, 0, None)), Next::Wait(Step::Checking));
}

#[test]
fn test_next_waits_for_a_first_check_until_the_head_goes_quiet() {
    let none = Checks::default();

    assert_eq!(
        next(&status(none, 0, Some(true))),
        Next::Wait(Step::WaitingForChecks)
    );
    assert_eq!(
        next(&Status {
            quiet: true,
            ..status(none, 0, Some(true))
        }),
        Next::Merge
    );
}
