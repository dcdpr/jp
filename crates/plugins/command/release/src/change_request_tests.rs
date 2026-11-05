use jp_test::mock::{MockServer, POST};
use serde_json::json;

use super::*;

fn request() -> ChangeRequest {
    ChangeRequest {
        branch: "release-cut/v0.3.0".to_owned(),
        title: "Cut release 0.3.0".to_owned(),
        body: "Files the notes of release 0.3.0.".to_owned(),
    }
}

fn labels() -> Vec<String> {
    vec!["no-changelog".to_owned()]
}

#[test]
fn github_takes_its_repository_from_origins_address() {
    let opener = Opener::new(
        Forge::GitHub,
        Some("https://github.com/dcdpr/jp"),
        Some("token".to_owned()),
        labels(),
    );

    assert_eq!(
        opener,
        Ok(Opener::GitHub {
            api: "https://api.github.com".to_owned(),
            owner: "dcdpr".to_owned(),
            repo: "jp".to_owned(),
            token: "token".to_owned(),
            labels: labels(),
        })
    );
}

#[test]
fn a_github_enterprise_server_serves_its_api_on_its_own_host() {
    let Ok(Opener::GitHub { api, .. }) = Opener::new(
        Forge::GitHub,
        Some("https://github.example.com/dcdpr/jp"),
        Some("token".to_owned()),
        vec![],
    ) else {
        panic!("expected a GitHub opener");
    };

    assert_eq!(api, "https://github.example.com/api/v3");
}

/// Everything opening a pull request needs is checked before anything is
/// pushed.
#[test]
fn github_needs_a_repository_and_a_token() {
    assert_eq!(
        Opener::new(
            Forge::GitHub,
            Some("https://github.com/dcdpr/jp"),
            None,
            vec![]
        ),
        Err(
            "Opening the pull request needs a GitHub token; set `JP_GITHUB_TOKEN` or \
             `GITHUB_TOKEN`."
                .to_owned()
        )
    );
    assert_eq!(
        Opener::new(Forge::GitHub, None, Some("token".to_owned()), vec![]),
        Err(
            "The forge is `github`, but git's `origin` remote has no web address to find the \
             repository at."
                .to_owned()
        )
    );
    assert_eq!(
        Opener::new(
            Forge::GitHub,
            Some("https://github.com/dcdpr"),
            Some("token".to_owned()),
            vec![]
        ),
        Err(
            "https://github.com/dcdpr, the address of git's `origin` remote, names no GitHub \
             repository."
                .to_owned()
        )
    );
}

#[test]
fn github_opens_a_pull_request_and_labels_it() {
    let server = MockServer::start();
    let create = server.mock(|when, then| {
        when.method(POST)
            .path("/repos/dcdpr/jp/pulls")
            .header("authorization", "Bearer token")
            .json_body(json!({
                "title": "Cut release 0.3.0",
                "head": "release-cut/v0.3.0",
                "base": "main",
                "body": "Files the notes of release 0.3.0."
            }));
        then.status(201).json_body(json!({
            "number": 1300,
            "html_url": "https://github.com/dcdpr/jp/pull/1300"
        }));
    });
    let label = server.mock(|when, then| {
        when.method(POST)
            .path("/repos/dcdpr/jp/issues/1300/labels")
            .json_body(json!({ "labels": ["no-changelog"] }));
        then.status(200)
            .json_body(json!([{ "name": "no-changelog", "description": null }]));
    });
    let opener = Opener::GitHub {
        api: server.base_url(),
        owner: "dcdpr".to_owned(),
        repo: "jp".to_owned(),
        token: "token".to_owned(),
        labels: labels(),
    };

    let opened = opener.open(&request(), "");

    assert_eq!(
        opened,
        Ok("Opened https://github.com/dcdpr/jp/pull/1300".to_owned())
    );
    create.assert();
    label.assert();
}

#[test]
fn a_pull_request_github_refuses_says_why() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(POST).path("/repos/dcdpr/jp/pulls");
        then.status(422)
            .json_body(json!({ "message": "Validation Failed" }));
    });
    let opener = Opener::GitHub {
        api: server.base_url(),
        owner: "dcdpr".to_owned(),
        repo: "jp".to_owned(),
        token: "token".to_owned(),
        labels: vec![],
    };

    let error = opener.open(&request(), "").unwrap_err();

    assert!(
        error.starts_with("Opening a pull request from release-cut/v0.3.0 failed: "),
        "{error}"
    );
    assert!(error.contains("Validation Failed"), "{error}");
}

#[test]
fn gitlab_opens_the_merge_request_with_push_options() {
    let opener = Opener::new(Forge::GitLab, None, None, labels()).unwrap();

    assert_eq!(opener.push_options(&request()), [
        "merge_request.create",
        "merge_request.target=main",
        "merge_request.title=Cut release 0.3.0",
        "merge_request.label=no-changelog",
    ]);
}

#[test]
fn gitlab_names_the_merge_request_the_push_opened() {
    let opener = Opener::GitLab { labels: vec![] };
    let pushed = "remote:\nremote: View merge request for release-cut/v0.3.0:\nremote:   \
                  https://gitlab.com/group/project/-/merge_requests/42\nremote:\nTo \
                  gitlab.com:group/project.git\n";

    assert_eq!(
        opener.open(&request(), pushed),
        Ok("Opened https://gitlab.com/group/project/-/merge_requests/42".to_owned())
    );
    assert_eq!(
        opener.open(&request(), ""),
        Ok("Asked GitLab to open a merge request from release-cut/v0.3.0 into main".to_owned())
    );
}

/// GitLab links to a new merge request when push options didn't open one.
#[test]
fn a_link_to_a_new_merge_request_is_not_one_that_opened() {
    let opener = Opener::GitLab { labels: vec![] };
    let pushed = "remote:\nremote: To create a merge request for release-cut/v0.3.0, visit:\nremote:   https://gitlab.com/group/project/-/merge_requests/new?merge_request%5Bsource_branch%5D=release-cut%2Fv0.3.0\nremote:\n";

    assert_eq!(
        opener.open(&request(), pushed),
        Ok("Asked GitLab to open a merge request from release-cut/v0.3.0 into main".to_owned())
    );
}

#[test]
fn without_a_forge_the_branch_is_left_to_merge() {
    let opener = Opener::new(Forge::None, None, None, labels()).unwrap();

    assert_eq!(opener.push_options(&request()), Vec::<String>::new());
    assert_eq!(
        opener.open(&request(), ""),
        Ok("Merge release-cut/v0.3.0 into main to file them".to_owned())
    );
}
