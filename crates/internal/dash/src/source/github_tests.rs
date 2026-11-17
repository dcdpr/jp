use std::sync::{Arc, Mutex};

use camino_tempfile::Utf8TempDir;

use super::*;
use crate::{
    model::StackPlace,
    queue::{QueueState, Reason},
};

const PULLS: &str = "repos/dcdpr/jp/pulls?state=open&per_page=100";

fn state() -> State {
    State {
        repo: Some("dcdpr/jp".to_owned()),
        rest: HashMap::from([(PULLS.to_owned(), Cached {
            etag: "W/\"01f8189bc680a1c9\"".to_owned(),
            body: "[]".to_owned(),
        })]),
        reviews: HashMap::from([(1132, Review {
            threads: 3,
            answered: 3,
            read_at: 1_790_000_000,
        })]),
        holds: HashMap::from([(1140, Hold {
            sha: "abc def".to_owned(),
            reason: Reason::Blocked("merging it failed: HTTP 405: no".to_owned()),
        })]),
    }
}

fn open(number: u64) -> PullRequest {
    PullRequest {
        number,
        title: String::new(),
        url: String::new(),
        head: String::new(),
        sha: String::new(),
        draft: false,
        checks: Checks::default(),
        threads: 0,
        answered: 0,
        labeled: false,
        base: None,
        stack: None,
        remote_branch: None,
        mergeable: None,
        queue: None,
    }
}

fn read_at(read_at: u64) -> Review {
    Review {
        threads: 1,
        answered: 0,
        read_at,
    }
}

#[test]
fn test_due_reviews_by_age_and_worktree() {
    let now = 10_000;
    let prs = [open(1), open(2), open(3), open(4), open(5)];
    // 1 and 2 have a local worktree; 3 and 4 do not; 5 was never read.
    let local = HashSet::from([1, 2]);
    let reviews = HashMap::from([
        (1, read_at(now - 60)),
        (2, read_at(now - 180)),
        (3, read_at(now - 600)),
        (4, read_at(now - 900)),
    ]);

    assert_eq!(due_reviews(&prs, &local, &reviews, now, false), [2, 4, 5]);
}

#[test]
fn test_due_reviews_all_when_asked() {
    let now = 10_000;
    let prs = [open(1), open(2)];
    let reviews = HashMap::from([(1, read_at(now)), (2, read_at(now))]);

    assert_eq!(
        due_reviews(&prs, &HashSet::new(), &reviews, now, false),
        Vec::<u64>::new()
    );
    assert_eq!(due_reviews(&prs, &HashSet::new(), &reviews, now, true), [
        1, 2
    ]);
}

fn github(state: State) -> GitHub {
    GitHub {
        dir: Utf8PathBuf::from("."),
        label: DEFAULT_QUEUE_LABEL.to_owned(),
        file: None,
        state,
        dirty: false,
        requested: HashSet::new(),
        default_branch: None,
        prefetched: HashMap::new(),
        stacks: true,
        driver: merge::Driver::default(),
    }
}

#[test]
fn test_state_survives_a_restart() {
    let dir = Utf8TempDir::new().unwrap();
    let file = dir.path().join("jp/dash/repo.github.json");

    cache::store(&file, &state()).unwrap();

    assert_eq!(cache::load::<State>(&file), state());
}

#[test]
fn test_offline_get_answers_from_the_cache_alone() {
    let mut github = github(state());

    assert_eq!(github.get(PULLS, false).unwrap(), "[]");
    assert!(matches!(
        github.get("repos/dcdpr/jp/commits/abc/check-runs", false),
        Err(Error::NotCached(path)) if path == "repos/dcdpr/jp/commits/abc/check-runs"
    ));
}

#[test]
fn test_cached_read_puts_labeled_pull_requests_in_the_queue() {
    let pull = |number: u64, labels: &str, base: &str| {
        format!(
            r#"{{"number": {number}, "title": "t", "html_url": "u", "labels": [{labels}],
               "head": {{"ref": "b{number}", "sha": "s{number}", "repo": {{"full_name": "dcdpr/jp"}}}},
               "base": {{"ref": "{base}", "repo": {{"full_name": "dcdpr/jp", "default_branch": "main"}}}}}}"#
        )
    };
    let queued = r#"{"name": "merge-when-green"}"#;
    let other = r#"{"name": "bug"}"#;
    let body = format!(
        "[{}, {}, {}]",
        pull(5, queued, "main"),
        pull(3, other, "main"),
        pull(8, queued, "feature"),
    );

    let cached = |body: String| Cached {
        etag: "e".to_owned(),
        body,
    };
    let mut state = state();
    state.rest.insert(PULLS.to_owned(), cached(body));
    for sha in ["s3", "s5", "s8"] {
        state.rest.insert(
            format!("repos/dcdpr/jp/commits/{sha}/check-runs?per_page=100"),
            cached(r#"{"check_runs": []}"#.to_owned()),
        );
        state.rest.insert(
            format!("repos/dcdpr/jp/commits/{sha}/status?per_page=100"),
            cached(r#"{"statuses": []}"#.to_owned()),
        );
    }

    let prs = github(state).cached().unwrap();
    let queue: Vec<_> = prs
        .iter()
        .map(|pr| (pr.number, pr.labeled, pr.base.clone(), pr.queue.clone()))
        .collect();

    assert_eq!(queue, [
        (3, false, None, None),
        (5, true, None, Some(QueueState::Waiting { position: 1 })),
        (
            8,
            true,
            Some("feature".to_owned()),
            Some(QueueState::OtherBase("feature".to_owned()))
        ),
    ]);
}

#[test]
fn test_fetch_takes_a_prefetched_answer_instead_of_calling_gh() {
    let mut github = github(state());
    let reply = |status, etag: Option<&str>, body: &str| {
        Ok(Reply {
            status,
            etag: etag.map(str::to_owned),
            body: body.to_owned(),
        })
    };
    github
        .prefetched
        .insert(PULLS.to_owned(), reply(304, None, ""));
    github.prefetched.insert(
        "repos/dcdpr/jp/commits/abc/status".to_owned(),
        reply(200, Some("W/\"new\""), r#"{"statuses": []}"#),
    );

    // Online, but nothing reaches `gh`: both answers were waiting.
    assert_eq!(github.get(PULLS, true).unwrap(), "[]");
    assert_eq!(
        github
            .get("repos/dcdpr/jp/commits/abc/status", true)
            .unwrap(),
        r#"{"statuses": []}"#
    );

    assert!(github.prefetched.is_empty());
    assert_eq!(
        github.etag("repos/dcdpr/jp/commits/abc/status"),
        Some("W/\"new\"")
    );
}

#[test]
fn test_parse_stacks_places_each_member_of_an_open_stack() {
    let stacks = r#"[
        {"number": 4, "open": true, "base": {"ref": "main"},
         "pull_requests": [
            {"number": 20, "head": {"sha": "a20"}},
            {"number": 21, "head": {"sha": "a21"}}
         ]},
        {"number": 3, "open": false, "base": {"ref": "main"},
         "pull_requests": [{"number": 9, "head": {"sha": "a9"}}]}
    ]"#;

    let places = parse_stacks(stacks).unwrap();

    assert_eq!(places.len(), 2);
    assert_eq!(
        places[&20],
        (
            StackPlace {
                stack: 4,
                position: 1,
                size: 2,
                branch: String::new(),
                below: None,
            },
            "main".to_owned()
        )
    );
    assert_eq!(places[&21].0.position, 2);
    assert_eq!(places[&21].0.below.as_deref(), Some("a20"));
}

#[test]
fn test_cached_read_places_stacked_pull_requests_on_the_stacks_base() {
    let pull = |number: u64, base: &str| {
        format!(
            r#"{{"number": {number}, "title": "t", "html_url": "u",
               "labels": [{{"name": "merge-when-green"}}],
               "head": {{"ref": "b{number}", "sha": "s{number}", "repo": {{"full_name": "dcdpr/jp"}}}},
               "base": {{"ref": "{base}", "repo": {{"full_name": "dcdpr/jp", "default_branch": "main"}}}}}}"#
        )
    };
    // #2 merges into #1's branch; the stack as a whole merges into `main`.
    let body = format!("[{}, {}]", pull(1, "main"), pull(2, "b1"));
    let stacks = r#"[{"number": 7, "open": true, "base": {"ref": "main"},
                      "pull_requests": [{"number": 1, "head": {"sha": "s1"}},
                                        {"number": 2, "head": {"sha": "s2"}}]}]"#;

    let cached = |body: String| Cached {
        etag: "e".to_owned(),
        body,
    };
    let mut state = state();
    state.rest.insert(PULLS.to_owned(), cached(body));
    state
        .rest
        .insert(stacks_path("dcdpr/jp"), cached(stacks.to_owned()));
    for sha in ["s1", "s2"] {
        state.rest.insert(
            runs_path("dcdpr/jp", sha),
            cached(r#"{"check_runs": []}"#.to_owned()),
        );
        state.rest.insert(
            status_path("dcdpr/jp", sha),
            cached(r#"{"statuses": []}"#.to_owned()),
        );
    }

    let prs = github(state).cached().unwrap();

    assert_eq!(prs[1].base, None);
    assert_eq!(
        prs[1].stack.as_ref().map(|place| place.branch.as_str()),
        Some("b2")
    );
    assert_eq!(prs[1].queue, Some(QueueState::Waiting { position: 1 }));
    assert_eq!(prs[0].queue, Some(QueueState::Waiting { position: 1 }));
}

#[test]
fn test_label_edits_reach_github_in_the_order_pressed() {
    let applied = Arc::new(Mutex::new(vec![]));
    let (done, finished) = mpsc::channel();

    let edits = {
        let applied = Arc::clone(&applied);
        editor(
            move |number, add| {
                // The add is slow: run side by side, the remove would land first.
                if add {
                    thread::sleep(Duration::from_millis(50));
                }
                applied.lock().unwrap().push((number, add));
                Ok(())
            },
            move |_, _, _| {
                let _gone = done.send(());
            },
        )
    };
    edits.send((7, true)).unwrap();
    edits.send((7, false)).unwrap();
    for _ in 0..2 {
        finished.recv_timeout(Duration::from_secs(5)).unwrap();
    }

    assert_eq!(*applied.lock().unwrap(), [(7, true), (7, false)]);
}

#[test]
fn test_the_queues_holds_are_saved_with_the_state() {
    let mut github = github(State::default());
    github.driver = merge::Driver::new(state().holds);

    github.keep_holds();

    assert!(github.dirty);
    assert_eq!(github.state.holds, state().holds);
}

#[test]
fn test_offline_read_without_a_cached_repository_name() {
    let mut github = github(State::default());

    assert!(matches!(github.repo(false), Err(Error::NotCached(_))));
    assert_eq!(github.cached(), None);
}

#[test]
fn test_parse_response_reads_status_etag_and_body() {
    let raw = "HTTP/2.0 200 OK\nContent-Type: application/json; charset=utf-8\nEtag: \
               W/\"01f8189bc680a1c9\"\nX-Ratelimit-Limit: 5000\nX-Ratelimit-Remaining: \
               4993\nX-Ratelimit-Used: 7\n\n[{\"number\":1132}]";

    assert_eq!(
        parse_response(raw),
        Some(Response {
            status: 200,
            etag: Some("W/\"01f8189bc680a1c9\""),
            body: "[{\"number\":1132}]",
        })
    );
}

#[test]
fn test_parse_response_not_modified_has_no_body() {
    let raw = "HTTP/2.0 304 Not Modified\r\nAccess-Control-Allow-Origin: *\r\nX-Ratelimit-Limit: \
               5000\r\nX-Ratelimit-Remaining: 4993\r\n";

    assert_eq!(
        parse_response(raw),
        Some(Response {
            status: 304,
            etag: None,
            body: "",
        })
    );
}

#[test]
fn test_parse_response_without_a_status_line() {
    // `gh` printed nothing to stdout: it failed before any response came back.
    assert_eq!(parse_response(""), None);
    assert_eq!(parse_response("not http\n\n{}"), None);
}

#[test]
fn test_api_message_prefers_the_error_message() {
    assert_eq!(
        api_message(r#"{"message": "API rate limit exceeded for user ID 383250."}"#),
        "API rate limit exceeded for user ID 383250."
    );
    assert_eq!(api_message("Bad Gateway"), "Bad Gateway");
}

#[test]
fn test_contexts_tally_check_runs_and_commit_statuses() {
    let runs = r#"{"total_count": 4, "check_runs": [
        {"name": "lint", "status": "completed", "conclusion": "success", "started_at": "2026-09-22T10:00:00Z"},
        {"name": "shear", "status": "completed", "conclusion": "skipped", "started_at": "2026-09-22T10:00:00Z"},
        {"name": "test", "status": "in_progress", "conclusion": null, "started_at": "2026-09-22T10:00:00Z"},
        {"name": "docs", "status": "completed", "conclusion": "failure", "started_at": "2026-09-22T10:00:00Z"}
    ]}"#;
    let status = r#"{"state": "pending", "statuses": [
        {"context": "coverage/coveralls", "state": "pending"}
    ]}"#;

    assert_eq!(checks(contexts(runs, status).unwrap(), &[]), Checks {
        passed: 2,
        pending: 2,
        failed: 1,
    });
}

#[test]
fn test_contexts_of_a_commit_without_checks() {
    let runs = r#"{"total_count": 0, "check_runs": []}"#;
    let status = r#"{"state": "pending", "statuses": []}"#;

    assert_eq!(
        checks(contexts(runs, status).unwrap(), &[]),
        Checks::default()
    );
}

#[test]
fn test_reviews_query_has_one_alias_per_pull_request() {
    assert_eq!(
        reviews_query(&[1132, 1193]),
        "query($owner: String!, $name: String!) {
  repository(owner: $owner, name: $name) {
    pr1132: pullRequest(number: 1132) { reviewThreads(first: 30) { totalCount nodes { \
         comments(first: 2) { nodes { id } } } } }
    pr1193: pullRequest(number: 1193) { reviewThreads(first: 30) { totalCount nodes { \
         comments(first: 2) { nodes { id } } } } }
  }
}"
    );
}

#[test]
fn test_parse_reviews_counts_threads_with_a_reply() {
    let json = r#"{"data": {
        "repository": {
            "pr1193": {"reviewThreads": {"totalCount": 0, "nodes": []}},
            "pr1132": {"reviewThreads": {"totalCount": 3, "nodes": [
                {"comments": {"nodes": [{"id": "a"}, {"id": "b"}]}},
                {"comments": {"nodes": [{"id": "c"}]}},
                {"comments": {"nodes": [{"id": "d"}, {"id": "e"}]}}
            ]}},
            "pr1200": null
        }
    }}"#;

    assert_eq!(
        parse_reviews(json).unwrap(),
        HashMap::from([(1132, (3, 2)), (1193, (0, 0))])
    );
}

#[test]
fn test_parse_reviews_counts_threads_past_the_ones_read() {
    // Two of the three threads were fetched (both answered); totalCount (3)
    // must still be preserved.
    let json = r#"{"data": {"repository": {
        "pr1132": {"reviewThreads": {"totalCount": 3, "nodes": [
            {"comments": {"nodes": [{"id": "a"}, {"id": "b"}]}},
            {"comments": {"nodes": [{"id": "c"}, {"id": "d"}]}}
        ]}}
    }}}"#;

    assert_eq!(
        parse_reviews(json).unwrap(),
        HashMap::from([(1132, (3, 2))])
    );
}

#[test]
fn test_local_branch_names_a_forks_default_branch_after_its_owner() {
    let pull = |head_ref: &str, head_repo: &str| -> RestPull {
        parse_json(&format!(
            r#"{{"number": 1, "title": "t", "html_url": "u",
                "head": {{"ref": "{head_ref}", "sha": "a", "user": {{"login": "octo"}}, "repo": {head_repo}}},
                "base": {{"repo": {{"full_name": "dcdpr/jp", "default_branch": "main"}}}}}}"#
        ))
        .unwrap()
    };
    let fork = r#"{"full_name": "octo/jp"}"#;
    let same = r#"{"full_name": "dcdpr/jp"}"#;

    assert_eq!(local_branch(&pull("main", fork)), "octo/main");
    assert_eq!(local_branch(&pull("feature", fork)), "feature");
    // A deleted fork still came from another repository.
    assert_eq!(local_branch(&pull("main", "null")), "octo/main");
    assert_eq!(local_branch(&pull("main", same)), "main");
}

#[test]
fn test_parse_reviews_rejects_a_graphql_error_response() {
    let json = r#"{"errors": [{"message": "API rate limit exceeded"}]}"#;

    assert!(matches!(parse_reviews(json).unwrap_err(), Error::Parse {
        program: "gh",
        ..
    }));
}

#[test]
fn test_parse_required_reads_only_required_status_check_rules() {
    let rules = r#"[
        {"type": "deletion"},
        {"type": "pull_request", "parameters": {"required_approving_review_count": 0}},
        {"type": "required_status_checks", "parameters": {
            "strict_required_status_checks_policy": false,
            "required_status_checks": [{"context": "lint"}, {"context": "test", "integration_id": 15368}]
        }}
    ]"#;

    assert_eq!(parse_required(rules).unwrap(), ["lint", "test"]);
    assert_eq!(parse_required("[]").unwrap(), Vec::<String>::new());
}

#[test]
fn test_required_checks_that_have_not_reported_count_as_pending() {
    let runs = r#"{"check_runs": [
        {"name": "lint", "status": "completed", "conclusion": "success", "started_at": "2026-01-01T00:00:00Z"}
    ]}"#;
    let status = r#"{"statuses": []}"#;
    let required = ["lint".to_owned(), "test".to_owned(), "build".to_owned()];

    assert_eq!(checks(contexts(runs, status).unwrap(), &required), Checks {
        passed: 1,
        pending: 2,
        failed: 0,
    });
}

#[test]
fn test_checks_count_only_the_latest_attempt_of_a_rerun() {
    let contexts = vec![
        Context::CheckRun {
            name: "test".to_owned(),
            status: "COMPLETED".to_owned(),
            conclusion: Some("FAILURE".to_owned()),
            started_at: Some("2026-09-22T10:00:00Z".to_owned()),
        },
        Context::CheckRun {
            name: "test".to_owned(),
            status: "COMPLETED".to_owned(),
            conclusion: Some("SUCCESS".to_owned()),
            started_at: Some("2026-09-22T11:00:00Z".to_owned()),
        },
    ];

    assert_eq!(checks(contexts, &[]), Checks {
        passed: 1,
        pending: 0,
        failed: 0,
    });
}

#[test]
fn test_checks_treat_an_unstarted_rerun_as_the_latest() {
    // The queued re-run has no start time yet, and must win over the failed
    // attempt whichever order GitHub lists them in.
    let failed = || Context::CheckRun {
        name: "test".to_owned(),
        status: "COMPLETED".to_owned(),
        conclusion: Some("FAILURE".to_owned()),
        started_at: Some("2026-09-22T10:00:00Z".to_owned()),
    };
    let queued = || Context::CheckRun {
        name: "test".to_owned(),
        status: "QUEUED".to_owned(),
        conclusion: None,
        started_at: None,
    };
    let pending = Checks {
        passed: 0,
        pending: 1,
        failed: 0,
    };

    assert_eq!(checks(vec![failed(), queued()], &[]), pending);
    assert_eq!(checks(vec![queued(), failed()], &[]), pending);
}
