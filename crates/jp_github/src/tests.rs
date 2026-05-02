use jp_test::mock::{DELETE, GET, MockServer, POST};
use serde_json::{Value, json};

use crate::{Error, Octocrab, params};

fn test_client(base_url: &str, token: Option<&str>) -> Octocrab {
    Octocrab::with_base_url(base_url, token)
}

fn issue_json(number: u64) -> Value {
    json!({
        "number": number,
        "title": format!("Issue #{number}"),
        "body": null,
        "html_url": format!("https://github.com/acme/widgets/issues/{number}"),
        "labels": [{"name": "bug", "description": null}],
        "user": {"login": "octocat"},
        "created_at": "2024-01-01T00:00:00Z",
        "closed_at": null,
        "pull_request": null
    })
}

fn pull_json(number: u64) -> Value {
    json!({
        "number": number,
        "title": format!("PR #{number}"),
        "body": null,
        "html_url": format!("https://github.com/acme/widgets/pull/{number}"),
        "labels": [{"name": "enhancement", "description": null}],
        "user": {"login": "octocat"},
        "created_at": "2024-01-01T00:00:00Z",
        "closed_at": null,
        "merged_at": null,
        "merge_commit_sha": null
    })
}

#[tokio::test]
async fn current_user_sends_auth_header_and_parses_response() {
    let server = MockServer::start_async().await;
    let mock = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/user")
                .header("authorization", "Bearer test-token");
            then.status(200).json_body(json!({ "login": "alice" }));
        })
        .await;

    let client = test_client(&server.base_url(), Some("test-token"));
    let user = client
        .current()
        .user()
        .await
        .expect("user request to succeed");
    assert_eq!(user.login, "alice");
    mock.assert();
}

#[tokio::test]
async fn current_user_maps_github_error_status_and_message() {
    let server = MockServer::start_async().await;
    let mock = server
        .mock_async(|when, then| {
            when.method(GET).path("/user");
            then.status(404)
                .json_body(json!({ "message": "Not Found" }));
        })
        .await;

    let client = test_client(&server.base_url(), None);
    let error = client
        .current()
        .user()
        .await
        .expect_err("request should fail");

    match error {
        Error::GitHub { source, body } => {
            assert_eq!(source.status_code, 404);
            assert_eq!(source.status_code.as_u16(), 404);
            assert_eq!(source.message, "Not Found");
            assert!(
                body.as_deref().is_some_and(|b| b.contains("Not Found")),
                "expected serialized github error body"
            );
        }
        other => panic!("unexpected error variant: {other:?}"),
    }

    mock.assert();
}

#[tokio::test]
async fn issues_list_paginates_across_pages() {
    let server = MockServer::start_async().await;
    let page_1 = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/repos/acme/widgets/issues")
                .query_param("per_page", "2")
                .query_param("page", "1");
            then.status(200)
                .json_body(json!([issue_json(1), issue_json(2)]));
        })
        .await;
    let page_2 = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/repos/acme/widgets/issues")
                .query_param("per_page", "2")
                .query_param("page", "2");
            then.status(200).json_body(json!([issue_json(3)]));
        })
        .await;

    let client = test_client(&server.base_url(), None);
    let page = client
        .issues("acme", "widgets")
        .list()
        .per_page(2)
        .send()
        .await
        .expect("list issues");

    let issues = client.all_pages(page).await.expect("all pages");
    assert_eq!(issues.len(), 3);
    assert_eq!(issues[0].number, 1);
    assert_eq!(issues[1].number, 2);
    assert_eq!(issues[2].number, 3);

    page_1.assert();
    page_2.assert();
}

#[tokio::test]
async fn issue_create_serializes_expected_payload() {
    let server = MockServer::start_async().await;
    let mock = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/repos/acme/widgets/issues")
                .json_body(json!({
                    "title": "Add tests",
                    "body": "Need coverage",
                    "labels": ["bug", "good first issue"],
                    "assignees": ["alice"]
                }));
            then.status(201).json_body(issue_json(42));
        })
        .await;

    let client = test_client(&server.base_url(), None);
    let issue = client
        .issues("acme", "widgets")
        .create("Add tests")
        .body("Need coverage")
        .labels(Some(vec!["bug".to_owned(), "good first issue".to_owned()]))
        .assignees(Some(vec!["alice".to_owned()]))
        .send()
        .await
        .expect("create issue");

    assert_eq!(issue.number, 42);
    mock.assert();
}

fn pending_review_json(id: u64, login: &str) -> Value {
    json!({
        "id": id,
        "node_id": format!("nid_{id}"),
        "user": {"login": login},
        "body": null,
        "state": "PENDING",
        "html_url": format!("https://github.com/acme/widgets/pull/7#pullrequestreview-{id}"),
        "submitted_at": null,
        "commit_id": "deadbeef"
    })
}

#[tokio::test]
async fn pulls_add_review_thread_posts_graphql_mutation() {
    use crate::models::pulls::{DraftReviewComment, Side};

    let server = MockServer::start_async().await;
    let mock = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/graphql")
                .body_includes("addPullRequestReviewThread")
                .body_includes("\"pullRequestReviewId\":\"R_kgDOABCDEFG\"")
                .body_includes("\"path\":\"src/lib.rs\"")
                .body_includes("\"line\":12")
                .body_includes("\"side\":\"RIGHT\"");
            then.status(200).json_body(json!({
                "data": {
                    "addPullRequestReviewThread": {
                        "thread": { "id": "PRRT_abc123" }
                    }
                }
            }));
        })
        .await;

    let client = test_client(&server.base_url(), None);
    client
        .pulls("acme", "widgets")
        .add_review_thread("R_kgDOABCDEFG", &DraftReviewComment {
            path: "src/lib.rs".to_owned(),
            body: "this needs fixing".to_owned(),
            line: 12,
            side: Some(Side::Right),
            start_line: None,
            start_side: None,
        })
        .await
        .expect("add review thread");

    mock.assert();
}

#[tokio::test]
async fn pulls_add_review_thread_surfaces_graphql_errors() {
    use crate::{Error, models::pulls::DraftReviewComment};

    let server = MockServer::start_async().await;
    let mock = server
        .mock_async(|when, then| {
            when.method(POST).path("/graphql");
            then.status(200).json_body(json!({
                "data": null,
                "errors": [{"message": "Pull request review not found"}]
            }));
        })
        .await;

    let client = test_client(&server.base_url(), None);
    let err = client
        .pulls("acme", "widgets")
        .add_review_thread("missing", &DraftReviewComment {
            path: "src/lib.rs".to_owned(),
            body: "x".to_owned(),
            line: 1,
            side: None,
            start_line: None,
            start_side: None,
        })
        .await
        .expect_err("GraphQL errors should surface");

    match err {
        Error::GitHub { source, .. } => {
            assert!(
                source.message.contains("Pull request review not found"),
                "unexpected error message: {}",
                source.message
            );
        }
        other => panic!("unexpected error variant: {other:?}"),
    }
    mock.assert();
}

#[tokio::test]
async fn pulls_create_review_serializes_pending_payload() {
    use crate::models::pulls::{DraftReviewComment, ReviewState, Side};

    let server = MockServer::start_async().await;
    let mock = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/repos/acme/widgets/pulls/7/reviews")
                .json_body(json!({
                    "body": "Drafted by jp",
                    "comments": [
                        {
                            "path": "src/lib.rs",
                            "body": "single line",
                            "line": 12,
                            "side": "RIGHT"
                        },
                        {
                            "path": "src/main.rs",
                            "body": "ranged",
                            "line": 30,
                            "side": "RIGHT",
                            "start_line": 25,
                            "start_side": "RIGHT"
                        }
                    ]
                }));
            then.status(200).json_body(pending_review_json(99, "alice"));
        })
        .await;

    let client = test_client(&server.base_url(), None);
    let review = client
        .pulls("acme", "widgets")
        .create_review(7)
        .body("Drafted by jp")
        .comments(vec![
            DraftReviewComment {
                path: "src/lib.rs".to_owned(),
                body: "single line".to_owned(),
                line: 12,
                side: Some(Side::Right),
                start_line: None,
                start_side: None,
            },
            DraftReviewComment {
                path: "src/main.rs".to_owned(),
                body: "ranged".to_owned(),
                line: 30,
                side: Some(Side::Right),
                start_line: Some(25),
                start_side: Some(Side::Right),
            },
        ])
        .send()
        .await
        .expect("create review");

    assert_eq!(review.id, 99);
    assert_eq!(review.state, ReviewState::Pending);
    mock.assert();
}

#[tokio::test]
async fn pulls_list_reviews_returns_pending_for_user() {
    let server = MockServer::start_async().await;
    let mock = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/repos/acme/widgets/pulls/7/reviews")
                .query_param("per_page", "100")
                .query_param("page", "1");
            then.status(200)
                .json_body(json!([pending_review_json(11, "alice")]));
        })
        .await;

    let client = test_client(&server.base_url(), None);
    let page = client
        .pulls("acme", "widgets")
        .list_reviews(7)
        .await
        .expect("list reviews");
    let reviews = client.all_pages(page).await.expect("all pages");

    assert_eq!(reviews.len(), 1);
    assert_eq!(reviews[0].id, 11);
    assert_eq!(
        reviews[0].user.as_ref().map(|u| u.login.as_str()),
        Some("alice")
    );
    mock.assert();
}

#[tokio::test]
#[allow(clippy::too_many_lines, reason = "large mock fixture")]
async fn pulls_fetch_review_comments_inherits_thread_side_and_outdated() {
    use crate::models::pulls::Side;

    let server = MockServer::start_async().await;
    let mock = server
        .mock_async(|when, then| {
            when.method(POST).path("/graphql");
            then.status(200).json_body(json!({
                "data": {
                    "repository": {
                        "pullRequest": {
                            "reviewThreads": {
                                "nodes": [
                                    // Live thread on the new file (RIGHT)
                                    // — `line` is set even though this is a
                                    // pending review comment.
                                    {
                                        "isOutdated": false,
                                        "diffSide": "RIGHT",
                                        "startDiffSide": null,
                                        "comments": {
                                            "nodes": [
                                                {
                                                    "fullDatabaseId": "100",
                                                    "path": "src/lib.rs",
                                                    "outdated": false,
                                                    "line": 12,
                                                    "startLine": null,
                                                    "originalLine": 12,
                                                    "originalStartLine": null,
                                                    "body": "still here",
                                                    "createdAt": "2024-01-01T00:00:00Z",
                                                    "author": { "login": "alice" },
                                                    "replyTo": null,
                                                    "pullRequestReview": { "fullDatabaseId": "7" }
                                                }
                                            ]
                                        }
                                    },
                                    // Outdated thread on the old file (LEFT),
                                    // multi-line. `line` is null — caller
                                    // can fall back to `original_line`.
                                    {
                                        "isOutdated": true,
                                        "diffSide": "LEFT",
                                        "startDiffSide": "LEFT",
                                        "comments": {
                                            "nodes": [
                                                {
                                                    "fullDatabaseId": "200",
                                                    "path": "src/main.rs",
                                                    "outdated": true,
                                                    "line": null,
                                                    "startLine": null,
                                                    "originalLine": 30,
                                                    "originalStartLine": 25,
                                                    "body": "force-pushed",
                                                    "createdAt": "2024-01-02T00:00:00Z",
                                                    "author": { "login": "alice" },
                                                    "replyTo": null,
                                                    "pullRequestReview": { "fullDatabaseId": "7" }
                                                },
                                                {
                                                    "fullDatabaseId": "201",
                                                    "path": "src/main.rs",
                                                    "outdated": true,
                                                    "line": null,
                                                    "startLine": null,
                                                    "originalLine": 30,
                                                    "originalStartLine": 25,
                                                    "body": "reply on outdated",
                                                    "createdAt": "2024-01-03T00:00:00Z",
                                                    "author": { "login": "bob" },
                                                    "replyTo": { "fullDatabaseId": "200" },
                                                    "pullRequestReview": { "fullDatabaseId": "7" }
                                                }
                                            ]
                                        }
                                    }
                                ]
                            }
                        }
                    }
                }
            }));
        })
        .await;

    let client = test_client(&server.base_url(), None);
    let comments = client
        .pulls("acme", "widgets")
        .fetch_review_comments(7)
        .await
        .expect("fetch review comments");

    assert_eq!(comments.len(), 3);

    let live = comments.iter().find(|c| c.id == 100).expect("live comment");
    assert!(!live.outdated);
    assert_eq!(live.line, Some(12));
    assert_eq!(live.side, Some(Side::Right));
    assert_eq!(live.path, "src/lib.rs");
    assert_eq!(live.user.as_ref().map(|u| u.login.as_str()), Some("alice"));

    let outdated_parent = comments.iter().find(|c| c.id == 200).expect("outdated");
    assert!(outdated_parent.outdated);
    assert_eq!(outdated_parent.line, None);
    assert_eq!(outdated_parent.original_line, Some(30));
    assert_eq!(outdated_parent.original_start_line, Some(25));
    assert_eq!(outdated_parent.side, Some(Side::Left));
    assert_eq!(outdated_parent.start_side, Some(Side::Left));

    let reply = comments.iter().find(|c| c.id == 201).expect("reply");
    assert!(reply.outdated, "reply inherits the thread's outdated flag");
    assert_eq!(reply.in_reply_to_id, Some(200));
    assert_eq!(reply.side, Some(Side::Left));

    mock.assert();
}

#[tokio::test]
async fn pulls_delete_review_treats_204_as_success() {
    let server = MockServer::start_async().await;
    let mock = server
        .mock_async(|when, then| {
            when.method(DELETE)
                .path("/repos/acme/widgets/pulls/7/reviews/11");
            then.status(204);
        })
        .await;

    let client = test_client(&server.base_url(), None);
    client
        .pulls("acme", "widgets")
        .delete_review(7, 11)
        .await
        .expect("delete pending review");
    mock.assert();
}

#[tokio::test]
async fn pulls_list_uses_state_query() {
    let server = MockServer::start_async().await;
    let mock = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/repos/acme/widgets/pulls")
                .query_param("state", "closed")
                .query_param("per_page", "100")
                .query_param("page", "1");
            then.status(200).json_body(json!([pull_json(12)]));
        })
        .await;

    let client = test_client(&server.base_url(), None);
    let page = client
        .pulls("acme", "widgets")
        .list()
        .state(params::State::Closed)
        .per_page(100)
        .send()
        .await
        .expect("list pulls");
    let pulls = client.all_pages(page).await.expect("all pages");

    assert_eq!(pulls.len(), 1);
    assert_eq!(pulls[0].number, 12);
    mock.assert();
}

#[tokio::test]
async fn repo_content_request_supports_ref_and_single_object_response() {
    let server = MockServer::start_async().await;
    let mock = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/repos/acme/widgets/contents/README.md")
                .query_param("ref", "main");
            then.status(200).json_body(json!({
                "path": "README.md",
                "type": "file",
                "content": "aGVsbG8=",
                "encoding": "base64"
            }));
        })
        .await;

    let client = test_client(&server.base_url(), None);
    let items = client
        .repos("acme", "widgets")
        .get_content()
        .path("README.md")
        .r#ref("main")
        .send()
        .await
        .expect("get content")
        .take_items();

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].path, "README.md");
    assert_eq!(items[0].r#type, "file");
    mock.assert();
}

#[tokio::test]
async fn collaborators_map_to_expected_shape() {
    let server = MockServer::start_async().await;
    let mock = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/repos/acme/widgets/collaborators")
                .query_param("per_page", "100")
                .query_param("page", "1");
            then.status(200).json_body(json!([{ "login": "alice" }]));
        })
        .await;

    let client = test_client(&server.base_url(), None);
    let page = client
        .repos("acme", "widgets")
        .list_collaborators()
        .send()
        .await
        .expect("list collaborators");
    let collaborators = client.all_pages(page).await.expect("all pages");

    assert_eq!(collaborators.len(), 1);
    assert_eq!(collaborators[0].author.login, "alice");
    mock.assert();
}

#[tokio::test]
async fn code_search_uses_items_payload_shape() {
    let server = MockServer::start_async().await;
    let mock = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/search/code")
                .query_param("q", "foo repo:acme/widgets")
                .query_param("per_page", "100")
                .query_param("page", "1");
            then.status(200).json_body(json!({
                "items": [
                    {"path": "src/lib.rs", "sha": "abc123"}
                ]
            }));
        })
        .await;

    let client = test_client(&server.base_url(), None);
    let page = client
        .search()
        .code("foo repo:acme/widgets")
        .send()
        .await
        .expect("search code");
    let items = client.all_pages(page).await.expect("all pages");

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].path, "src/lib.rs");
    assert_eq!(items[0].sha, "abc123");
    mock.assert();
}

#[tokio::test]
async fn graphql_posts_json_to_graphql_endpoint() {
    let server = MockServer::start_async().await;
    let mock = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/graphql")
                .json_body(json!({"query": "{ viewer { login } }"}));
            then.status(200).json_body(json!({
                "data": {
                    "viewer": {
                        "login": "alice"
                    }
                }
            }));
        })
        .await;

    let client = test_client(&server.base_url(), None);
    let result: Value = client
        .graphql(&json!({"query": "{ viewer { login } }"}))
        .await
        .expect("graphql request");

    assert_eq!(result["data"]["viewer"]["login"], "alice");
    mock.assert();
}
