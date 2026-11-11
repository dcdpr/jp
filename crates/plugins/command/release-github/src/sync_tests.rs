use jp_test::mock::{GET, MockServer, PATCH};
use serde_json::{Value, json};

use super::*;

fn release_json(id: u64, tag: &str, body: &str) -> Value {
    json!({
        "id": id,
        "tag_name": tag,
        "name": tag,
        "body": body,
        "draft": false,
        "prerelease": false,
        "html_url": format!("https://github.com/acme/widgets/releases/tag/{tag}"),
        "upload_url": "https://uploads.github.com/repos/acme/widgets/releases/1/assets{?name,label}",
        "assets": [],
    })
}

/// Only the Release whose notes changed is rewritten: one that reads the same
/// with GitHub's line endings is left, and so are a Release of a tag that is no
/// version and one of a version without notes.
#[tokio::test]
async fn only_descriptions_that_differ_from_their_notes_are_rewritten() {
    let server = MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(GET).path("/repos/acme/widgets/releases");
            then.status(200).json_body(json!([
                release_json(9, "plugins", "The plugins."),
                release_json(3, "v0.3.0", "Old notes.\n"),
                release_json(2, "v0.2.0", "Same notes.\r\n"),
                release_json(1, "v0.1.0", "Notes from before."),
            ]));
        })
        .await;
    let rewrite = server
        .mock_async(|when, then| {
            when.method(PATCH)
                .path("/repos/acme/widgets/releases/3")
                .json_body(json!({ "body": "New notes.\n" }));
            then.status(200)
                .json_body(release_json(3, "v0.3.0", "New notes.\n"));
        })
        .await;
    let client = Octocrab::builder()
        .personal_token("token")
        .base_url(server.base_url())
        .build()
        .unwrap();
    let repository = Repository::parse("https://github.com/acme/widgets").unwrap();

    let mut described = vec![];
    let updated = sync(&client, &repository, &mut |version| {
        described.push(version.to_string());
        Ok(match version.minor {
            3 => Some("New notes.\n".to_owned()),
            2 => Some("Same notes.\n".to_owned()),
            _ => None,
        })
    })
    .await
    .unwrap();

    assert_eq!(updated, [Version::new(0, 3, 0)]);
    assert_eq!(described, ["0.3.0", "0.2.0", "0.1.0"]);
    rewrite.assert();
}

#[test]
fn a_sync_says_which_descriptions_it_rewrote() {
    assert_eq!(
        summary(&[]),
        "Every Release's description matches its notes.\n"
    );
    assert_eq!(
        summary(&[Version::new(0, 3, 0)]),
        "Updated the description of 0.3.0 to match its notes.\n"
    );
    assert_eq!(
        summary(&[
            Version::new(0, 1, 0),
            Version::new(0, 2, 0),
            Version::new(0, 3, 0),
        ]),
        "Updated the descriptions of 0.1.0, 0.2.0 and 0.3.0 to match their notes.\n"
    );
}
