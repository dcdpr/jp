use camino_tempfile::Utf8TempDir;
use indoc::indoc;
use jp_test::mock::{DELETE, GET, MockServer, PATCH, POST};
use serde_json::{Value, json};

use super::*;

/// The commit the builds in these tests are of.
const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

/// The archive the builds in these tests hold, and its SHA-256.
const ARCHIVE: &str = "jp-aarch64-apple-darwin.tar.xz";
const CONTENTS: &str = "an archive\n";
const HASH: &str = "4cd36b3ca046f7c17e81e1aa31bf2dc6a91b98c94238810db57456b4904d3a22";

/// Keep a build of 0.3.0 made with `profile` in `dir`, as `jp release build`
/// would.
fn keep_build(dir: &Utf8Path, profile: &str) {
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join(ARCHIVE), CONTENTS).unwrap();
    fs::write(dir.join(CHECKSUMS), format!("{HASH}  {ARCHIVE}\n")).unwrap();
    fs::write(
        dir.join(MANIFEST),
        json!({
            "format": 1,
            "version": "0.3.0",
            "commit": COMMIT,
            "profile": profile,
            "artifacts": [{ "file": ARCHIVE, "target": "aarch64-apple-darwin" }],
        })
        .to_string(),
    )
    .unwrap();
}

#[test]
fn a_publishable_build_is_its_archives_checksums_and_manifest() {
    let root = Utf8TempDir::new().unwrap();
    let dir = root.path().join("0.3.0");
    keep_build(&dir, "release");

    let (commit, uploads) = read_build(&dir, &Version::new(0, 3, 0)).unwrap();

    assert_eq!(commit, COMMIT);
    assert_eq!(
        uploads
            .iter()
            .map(|upload| upload.name.as_str())
            .collect::<Vec<_>>(),
        [ARCHIVE, CHECKSUMS, MANIFEST]
    );
    assert_eq!(uploads[0].contents, CONTENTS.as_bytes());
}

#[test]
fn without_a_build_there_is_nothing_to_publish() {
    let root = Utf8TempDir::new().unwrap();

    assert_eq!(
        read_build(&root.path().join("0.3.0"), &Version::new(0, 3, 0)),
        Err("No build of 0.3.0 is kept here; build one with `jp release build 0.3.0`.".to_owned())
    );
}

#[test]
fn a_quick_build_is_not_published() {
    let root = Utf8TempDir::new().unwrap();
    let dir = root.path().join("0.3.0");
    keep_build(&dir, "quick");

    assert_eq!(
        read_build(&dir, &Version::new(0, 3, 0)),
        Err(
            "The build of 0.3.0 kept here was made with the quick profile, which isn't published; \
             build it with `jp release build 0.3.0`."
                .to_owned()
        )
    );
}

/// An archive that changed after its build isn't the one its checksum vouches
/// for, so nothing is published.
#[test]
fn a_build_that_changed_since_it_was_made_is_not_published() {
    let root = Utf8TempDir::new().unwrap();
    let dir = root.path().join("0.3.0");
    keep_build(&dir, "release");
    fs::write(dir.join(ARCHIVE), "another archive\n").unwrap();

    let error = read_build(&dir, &Version::new(0, 3, 0)).unwrap_err();

    assert!(
        error.starts_with(
            "The build of 0.3.0 kept here changed since it was made, so build it again with `jp \
             release build 0.3.0`:\n- "
        ),
        "{error}"
    );
    assert!(error.contains(ARCHIVE), "{error}");
}

#[test]
fn assets_carry_the_releases_download_address() {
    let root = Utf8TempDir::new().unwrap();
    fs::write(root.path().join("install.sh"), "published='{download}'\n").unwrap();
    fs::write(root.path().join("logo.png"), [0xff, 0xd8, 0x7b]).unwrap();

    let uploads = assets(
        root.path(),
        &["install.sh", "logo.png"],
        "https://github.com/dcdpr/jp/releases/download/v0.3.0",
    )
    .unwrap();

    assert_eq!(uploads, [
        Upload {
            name: "install.sh".to_owned(),
            contents: b"published='https://github.com/dcdpr/jp/releases/download/v0.3.0'\n"
                .to_vec(),
        },
        Upload {
            name: "logo.png".to_owned(),
            contents: vec![0xff, 0xd8, 0x7b],
        },
    ]);
}

#[test]
fn an_asset_that_is_not_there_is_named() {
    let root = Utf8TempDir::new().unwrap();

    let error = assets(
        root.path(),
        &[".config/release/install.sh"],
        "https://example.com",
    )
    .unwrap_err();

    assert!(
        error.starts_with(
            "`.config/release/install.sh`, which `plugins.command.release-github.options.assets` \
             lists, can't be read: "
        ),
        "{error}"
    );
}

#[test]
fn two_files_of_one_name_are_refused() {
    let upload = |name: &str| Upload {
        name: name.to_owned(),
        contents: vec![],
    };

    assert_eq!(
        distinct(&[upload(ARCHIVE), upload(CHECKSUMS), upload(CHECKSUMS)]),
        Err(
            "Two files would be attached as `checksums.txt`; rename one of the files \
             `plugins.command.release-github.options.assets` lists."
                .to_owned()
        )
    );
    assert_eq!(distinct(&[upload(ARCHIVE), upload(CHECKSUMS)]), Ok(()));
}

fn release(id: u64, tag: &str, draft: bool, prerelease: bool) -> Release {
    Release {
        id,
        tag_name: tag.to_owned(),
        name: None,
        body: None,
        draft,
        prerelease,
        html_url: format!("https://github.com/acme/widgets/releases/tag/{tag}"),
        upload_url: String::new(),
        assets: vec![],
    }
}

/// Drafts and pre-releases aren't anyone's latest release, and the Release
/// being published doesn't compete with itself.
#[test]
fn the_highest_published_release_is_the_one_to_be_latest() {
    let releases = [
        release(9, "plugins", false, false),
        release(6, "v0.6.0", true, false),
        release(5, "v0.5.0", false, true),
        release(7, "v0.7.0-rc.1", false, false),
        release(4, "v0.4.0", false, false),
        release(3, "v0.3.0", false, false),
        release(2, "v0.2.0", false, false),
    ];

    assert_eq!(
        highest(&releases, "v0.4.0"),
        Some((Version::new(0, 3, 0), 3))
    );
    assert_eq!(
        highest(&releases, "v0.3.1"),
        Some((Version::new(0, 4, 0), 4))
    );
    assert_eq!(highest(&releases[..3], "v0.3.1"), None);
}

/// A Release on the mock server, which uploads to the server too.
fn release_json(base: &str, id: u64, tag: &str, draft: bool, assets: &Value) -> Value {
    json!({
        "id": id,
        "tag_name": tag,
        "name": format!("Release {}", tag.trim_start_matches('v')),
        "body": "The notes.\n",
        "draft": draft,
        "prerelease": false,
        "html_url": format!("https://github.com/acme/widgets/releases/tag/{tag}"),
        "upload_url": format!("{base}/uploads/repos/acme/widgets/releases/{id}/assets{{?name,label}}"),
        "assets": assets,
    })
}

fn asset_json(id: u64, name: &str) -> Value {
    json!({
        "id": id,
        "name": name,
        "browser_download_url": format!("https://github.com/acme/widgets/releases/download/v0.3.0/{name}"),
    })
}

fn client(server: &MockServer) -> Octocrab {
    Octocrab::builder()
        .personal_token("token")
        .base_url(server.base_url())
        .build()
        .unwrap()
}

fn repository() -> Repository {
    Repository::parse("https://github.com/acme/widgets").unwrap()
}

fn plan(version: Version) -> Plan {
    Plan {
        version,
        commit: COMMIT.to_owned(),
        body: "The notes.\n".to_owned(),
        uploads: vec![
            Upload {
                name: ARCHIVE.to_owned(),
                contents: CONTENTS.as_bytes().to_vec(),
            },
            Upload {
                name: CHECKSUMS.to_owned(),
                contents: format!("{HASH}  {ARCHIVE}\n").into_bytes(),
            },
        ],
    }
}

/// Answer the tag check for `tag` with `commit`.
async fn tag_at(server: &MockServer, tag: &str, commit: &str) {
    server
        .mock_async(|when, then| {
            when.method(GET)
                .path(format!("/repos/acme/widgets/commits/tags/{tag}"));
            then.status(200).body(commit);
        })
        .await;
}

/// Answer the listing of Releases with `releases`.
async fn releases_are(server: &MockServer, releases: Value) {
    server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/repos/acme/widgets/releases")
                .query_param("page", "1");
            then.status(200).json_body(releases);
        })
        .await;
}

/// Accept each of `names` uploaded to the Release with `id`.
async fn uploads_to(server: &MockServer, id: u64, names: &[&str]) {
    for (index, name) in (100..).zip(names) {
        server
            .mock_async(|when, then| {
                when.method(POST)
                    .path(format!("/uploads/repos/acme/widgets/releases/{id}/assets"))
                    .query_param("name", *name);
                then.status(201).json_body(asset_json(index, name));
            })
            .await;
    }
}

/// A new Release starts as a draft, so it never shows without its files, and
/// the highest version becomes the latest as it is published.
#[tokio::test]
async fn a_new_release_is_drafted_given_its_files_and_published_as_the_latest() {
    let server = MockServer::start_async().await;
    let base = server.base_url();
    tag_at(&server, "v0.3.0", COMMIT).await;
    releases_are(
        &server,
        json!([release_json(&base, 2, "v0.2.0", false, &json!([]))]),
    )
    .await;
    let create = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/repos/acme/widgets/releases")
                .json_body(json!({
                    "tag_name": "v0.3.0",
                    "name": "Release 0.3.0",
                    "body": "The notes.\n",
                    "draft": true,
                }));
            then.status(201)
                .json_body(release_json(&base, 3, "v0.3.0", true, &json!([])));
        })
        .await;
    uploads_to(&server, 3, &[ARCHIVE, CHECKSUMS]).await;
    let publish_it = server
        .mock_async(|when, then| {
            when.method(PATCH)
                .path("/repos/acme/widgets/releases/3")
                .json_body(json!({ "draft": false, "make_latest": "true" }));
            then.status(200)
                .json_body(release_json(&base, 3, "v0.3.0", false, &json!([])));
        })
        .await;

    let mut attached = vec![];
    let published = publish(
        &client(&server),
        &repository(),
        plan(Version::new(0, 3, 0)),
        &mut |name| attached.push(name.to_owned()),
    )
    .await
    .unwrap();

    assert_eq!(attached, [ARCHIVE, CHECKSUMS]);
    assert_eq!(published, Published {
        url: "https://github.com/acme/widgets/releases/tag/v0.3.0".to_owned(),
        created: true,
        uploaded: vec![ARCHIVE.to_owned(), CHECKSUMS.to_owned()],
        replaced: vec![],
        kept: vec![],
        latest: Version::new(0, 3, 0),
    });
    create.assert();
    publish_it.assert();
}

/// A patch release to an older line, published after a newer release, leaves
/// the newer one the latest.
#[tokio::test]
async fn an_older_release_is_published_without_becoming_the_latest() {
    let server = MockServer::start_async().await;
    let base = server.base_url();
    tag_at(&server, "v0.3.1", COMMIT).await;
    releases_are(
        &server,
        json!([release_json(&base, 4, "v0.4.0", false, &json!([]))]),
    )
    .await;
    server
        .mock_async(|when, then| {
            when.method(POST).path("/repos/acme/widgets/releases");
            then.status(201)
                .json_body(release_json(&base, 5, "v0.3.1", true, &json!([])));
        })
        .await;
    uploads_to(&server, 5, &[ARCHIVE, CHECKSUMS]).await;
    let publish_it = server
        .mock_async(|when, then| {
            when.method(PATCH)
                .path("/repos/acme/widgets/releases/5")
                .json_body(json!({ "draft": false, "make_latest": "false" }));
            then.status(200)
                .json_body(release_json(&base, 5, "v0.3.1", false, &json!([])));
        })
        .await;
    let keep_latest = server
        .mock_async(|when, then| {
            when.method(PATCH)
                .path("/repos/acme/widgets/releases/4")
                .json_body(json!({ "make_latest": "true" }));
            then.status(200)
                .json_body(release_json(&base, 4, "v0.4.0", false, &json!([])));
        })
        .await;

    let published = publish(
        &client(&server),
        &repository(),
        plan(Version::new(0, 3, 1)),
        &mut |_| {},
    )
    .await
    .unwrap();

    assert_eq!(published.latest, Version::new(0, 4, 0));
    publish_it.assert();
    keep_latest.assert();
}

/// Publishing again updates the Release in place: files this publish attaches
/// replace the ones of the same name, and files of other names stay.
#[tokio::test]
async fn publishing_again_updates_the_release_and_replaces_its_files() {
    let server = MockServer::start_async().await;
    let base = server.base_url();
    let assets = json!([asset_json(7, CHECKSUMS), asset_json(8, "old.zip")]);
    tag_at(&server, "v0.3.0", COMMIT).await;
    releases_are(
        &server,
        json!([release_json(&base, 3, "v0.3.0", false, &assets)]),
    )
    .await;
    let update = server
        .mock_async(|when, then| {
            when.method(PATCH)
                .path("/repos/acme/widgets/releases/3")
                .json_body(json!({ "name": "Release 0.3.0", "body": "The notes.\n" }));
            then.status(200)
                .json_body(release_json(&base, 3, "v0.3.0", false, &assets));
        })
        .await;
    let delete = server
        .mock_async(|when, then| {
            when.method(DELETE)
                .path("/repos/acme/widgets/releases/assets/7");
            then.status(204);
        })
        .await;
    uploads_to(&server, 3, &[ARCHIVE, CHECKSUMS]).await;
    let latest = server
        .mock_async(|when, then| {
            when.method(PATCH)
                .path("/repos/acme/widgets/releases/3")
                .json_body(json!({ "make_latest": "true" }));
            then.status(200)
                .json_body(release_json(&base, 3, "v0.3.0", false, &json!([])));
        })
        .await;

    let published = publish(
        &client(&server),
        &repository(),
        plan(Version::new(0, 3, 0)),
        &mut |_| {},
    )
    .await
    .unwrap();

    assert!(!published.created);
    assert_eq!(published.replaced, [CHECKSUMS]);
    assert_eq!(published.kept, ["old.zip"]);
    update.assert();
    delete.assert();
    latest.assert();
}

/// A Release of a tag GitHub doesn't have would make the tag, at the tip of the
/// default branch, so nothing is asked of GitHub but the check.
#[tokio::test]
async fn a_tag_github_does_not_have_is_refused() {
    let server = MockServer::start_async().await;
    server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/repos/acme/widgets/commits/tags/v0.3.0");
            then.status(422)
                .json_body(json!({ "message": "No commit found for SHA: tags/v0.3.0" }));
        })
        .await;

    let error = publish(
        &client(&server),
        &repository(),
        plan(Version::new(0, 3, 0)),
        &mut |_| {},
    )
    .await
    .unwrap_err();

    assert_eq!(
        error,
        "https://github.com/acme/widgets has no tag v0.3.0; push it first, with the `git push` \
         that `jp release finalize` printed."
    );
}

#[tokio::test]
async fn a_tag_at_another_commit_than_the_builds_is_refused() {
    let server = MockServer::start_async().await;
    tag_at(
        &server,
        "v0.3.0",
        "1111111111111111111111111111111111111111",
    )
    .await;

    let error = publish(
        &client(&server),
        &repository(),
        plan(Version::new(0, 3, 0)),
        &mut |_| {},
    )
    .await
    .unwrap_err();

    assert_eq!(
        error,
        "v0.3.0 on https://github.com/acme/widgets names commit 1111111, but the build kept here \
         is of 0123456; build it again with `jp release build 0.3.0`."
    );
}

#[test]
fn a_new_release_says_where_it_is_and_what_it_holds() {
    let published = Published {
        url: "https://github.com/acme/widgets/releases/tag/v0.3.0".to_owned(),
        created: true,
        uploaded: vec![ARCHIVE.to_owned(), CHECKSUMS.to_owned()],
        replaced: vec![],
        kept: vec![],
        latest: Version::new(0, 3, 0),
    };

    assert_eq!(summary(&published, &Version::new(0, 3, 0)), indoc! {"
        Published 0.3.0 at https://github.com/acme/widgets/releases/tag/v0.3.0, with 2 files:

            jp-aarch64-apple-darwin.tar.xz
            checksums.txt

        0.3.0 is the latest release.
    "});
}

#[test]
fn an_update_says_what_it_replaced_and_left() {
    let published = Published {
        url: "https://github.com/acme/widgets/releases/tag/v0.3.1".to_owned(),
        created: false,
        uploaded: vec![ARCHIVE.to_owned()],
        replaced: vec![ARCHIVE.to_owned()],
        kept: vec!["old.zip".to_owned(), "notes.txt".to_owned()],
        latest: Version::new(0, 4, 0),
    };

    assert_eq!(summary(&published, &Version::new(0, 3, 1)), indoc! {"
        Updated 0.3.1 at https://github.com/acme/widgets/releases/tag/v0.3.1, with 1 file:

            jp-aarch64-apple-darwin.tar.xz

        Replaced `jp-aarch64-apple-darwin.tar.xz`, which an earlier publish attached.

        Left `old.zip` and `notes.txt` attached, which this build doesn't have.

        0.4.0 stays the latest release, as the highest version.
    "});
}
