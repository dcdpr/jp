use camino_tempfile::Utf8TempDir;

use super::{label::DEFAULT_QUEUE_LABEL, *};
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
