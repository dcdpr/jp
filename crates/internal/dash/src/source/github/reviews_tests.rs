use super::*;
use crate::model::Checks;

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
fn test_parse_reviews_rejects_a_graphql_error_response() {
    let json = r#"{"errors": [{"message": "API rate limit exceeded"}]}"#;

    assert!(matches!(parse_reviews(json).unwrap_err(), Error::Parse {
        program: "gh",
        ..
    }));
}
