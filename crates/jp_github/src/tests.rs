use jp_test::mock::{GET, MockServer, POST};
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
