use camino_tempfile::Utf8TempDir;

use super::*;

fn jp() -> Repository {
    Repository {
        web: "https://github.com/dcdpr/jp".to_owned(),
        host: "github.com".to_owned(),
        owner: "dcdpr".to_owned(),
        name: "jp".to_owned(),
    }
}

#[test]
fn a_repository_reads_from_its_web_address() {
    assert_eq!(
        Repository::parse("https://github.com/dcdpr/jp/"),
        Some(jp())
    );
}

#[test]
fn an_address_that_names_no_single_repository_is_none() {
    assert_eq!(Repository::parse("https://github.com/dcdpr"), None);
    assert_eq!(
        Repository::parse("https://github.com/dcdpr/jp/tree/main"),
        None
    );
    assert_eq!(Repository::parse("https://github.com//jp"), None);
    assert_eq!(Repository::parse("git@github.com:dcdpr/jp.git"), None);
}

#[test]
fn github_itself_is_reached_at_its_default_api() {
    assert_eq!(jp().api(), None);
}

#[test]
fn github_enterprise_serves_its_api_below_api_v3() {
    let repository = Repository::parse("https://git.example.com/acme/widgets").unwrap();

    assert_eq!(
        repository.api().as_deref(),
        Some("https://git.example.com/api/v3")
    );
}

#[test]
fn a_releases_files_are_served_below_its_tag() {
    assert_eq!(
        jp().download("v0.3.0"),
        "https://github.com/dcdpr/jp/releases/download/v0.3.0"
    );
}

#[test]
fn the_configured_repository_is_the_one_published_to() {
    let root = Utf8TempDir::new().unwrap();

    assert_eq!(
        Repository::find(root.path(), Some("https://github.com/dcdpr/jp")),
        Ok(jp())
    );
}

#[test]
fn without_a_repository_or_an_origin_there_is_nowhere_to_publish() {
    let root = Utf8TempDir::new().unwrap();

    let error = Repository::find(root.path(), None).unwrap_err();

    assert!(
        error.contains("`plugins.command.release.options.repository`"),
        "{error}"
    );
}
