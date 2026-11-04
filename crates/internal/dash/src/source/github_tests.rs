use camino_tempfile::Utf8TempDir;

use super::*;

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
    }
}

fn open(number: u64) -> PullRequest {
    PullRequest {
        number,
        title: String::new(),
        url: String::new(),
        head: String::new(),
        draft: false,
        checks: Checks::default(),
        threads: 0,
        answered: 0,
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

/// A reader over `state` that has no cache file.
fn github(state: State) -> GitHub {
    GitHub {
        dir: Utf8PathBuf::from("."),
        file: None,
        state,
        dirty: false,
        requested: HashSet::new(),
    }
}

#[test]
fn test_state_survives_a_restart() {
    let dir = Utf8TempDir::new().unwrap();
    let file = dir.path().join("jp/dash/repo.json");

    store_state(&file, &state()).unwrap();

    assert_eq!(load_state(&file), state());
}

#[test]
fn test_a_missing_or_broken_cache_is_empty() {
    let dir = Utf8TempDir::new().unwrap();
    let file = dir.path().join("repo.json");
    assert_eq!(load_state(&file), State::default());

    fs::write(&file, "{ not json").unwrap();
    assert_eq!(load_state(&file), State::default());
}

#[test]
fn test_cache_home_follows_xdg() {
    let home = Some("/Users/jean".to_owned());

    assert_eq!(
        cache_home(Some("/tmp/cache".to_owned()), home.clone()),
        Some(Utf8PathBuf::from("/tmp/cache"))
    );
    // A relative `XDG_CACHE_HOME` is invalid and ignored.
    assert_eq!(
        cache_home(Some("cache".to_owned()), home.clone()),
        Some(Utf8PathBuf::from("/Users/jean/.cache"))
    );
    assert_eq!(
        cache_home(None, home),
        Some(Utf8PathBuf::from("/Users/jean/.cache"))
    );
    assert_eq!(cache_home(None, None), None);
}

#[test]
fn test_cache_path_names_the_file_after_the_repository() {
    assert_eq!(
        cache_path(
            Utf8Path::new("/Users/jean/.cache"),
            Utf8Path::new("/Users/jean/Projects/Personal/jp.git/.bare")
        ),
        Utf8PathBuf::from(
            "/Users/jean/.cache/jp/dash/Users-jean-Projects-Personal-jp-git--bare.json"
        )
    );
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

    assert_eq!(checks(contexts(runs, status).unwrap()), Checks {
        passed: 2,
        pending: 2,
        failed: 1,
    });
}

#[test]
fn test_contexts_of_a_commit_without_checks() {
    let runs = r#"{"total_count": 0, "check_runs": []}"#;
    let status = r#"{"state": "pending", "statuses": []}"#;

    assert_eq!(checks(contexts(runs, status).unwrap()), Checks::default());
}

#[test]
fn test_reviews_query_has_one_alias_per_pull_request() {
    assert_eq!(
        reviews_query(&[1132, 1193]),
        "query($owner: String!, $name: String!) {
  repository(owner: $owner, name: $name) {
    pr1132: pullRequest(number: 1132) { reviewThreads(first: 30) { nodes { comments(first: 2) { \
         nodes { id } } } } }
    pr1193: pullRequest(number: 1193) { reviewThreads(first: 30) { nodes { comments(first: 2) { \
         nodes { id } } } } }
  }
}"
    );
}

#[test]
fn test_parse_reviews_counts_threads_with_a_reply() {
    let json = r#"{"data": {
        "repository": {
            "pr1193": {"reviewThreads": {"nodes": []}},
            "pr1132": {"reviewThreads": {"nodes": [
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
fn test_parse_reviews_rejects_a_graphql_error_response() {
    let json = r#"{"errors": [{"message": "API rate limit exceeded"}]}"#;

    assert!(matches!(parse_reviews(json).unwrap_err(), Error::Parse {
        program: "gh",
        ..
    }));
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

    assert_eq!(checks(contexts), Checks {
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

    assert_eq!(checks(vec![failed(), queued()]), pending);
    assert_eq!(checks(vec![queued(), failed()]), pending);
}
