use super::*;

#[test]
fn parse_repo_accepts_standard_owner_repo() {
    let (owner, repo) = parse_repo(Some("Swatinem/rust-cache".to_owned())).unwrap();
    assert_eq!(owner, "Swatinem");
    assert_eq!(repo, "rust-cache");
}

#[test]
fn parse_repo_defaults_to_project_repo_when_unset() {
    let (owner, repo) = parse_repo(None).unwrap();
    assert_eq!(owner, ORG);
    assert_eq!(repo, REPO);
}

#[test]
fn parse_repo_rejects_missing_separator() {
    let err = parse_repo(Some("owner".to_owned())).unwrap_err();
    assert!(err.to_string().contains("<owner>/<repo>"));
}

#[test]
fn parse_repo_rejects_three_segments() {
    // `owner/repo/extra` would have interpolated into a malformed API
    // path and surfaced as a 404 instead of a friendly argument error.
    let err = parse_repo(Some("owner/repo/extra".to_owned())).unwrap_err();
    assert!(err.to_string().contains("<owner>/<repo>"));
}

#[test]
fn parse_repo_rejects_empty_owner() {
    let err = parse_repo(Some("/repo".to_owned())).unwrap_err();
    assert!(err.to_string().contains("<owner>/<repo>"));
}

#[test]
fn parse_repo_rejects_empty_repo() {
    let err = parse_repo(Some("owner/".to_owned())).unwrap_err();
    assert!(err.to_string().contains("<owner>/<repo>"));
}

#[test]
fn parse_repo_rejects_empty_string() {
    let err = parse_repo(Some(String::new())).unwrap_err();
    assert!(err.to_string().contains("<owner>/<repo>"));
}
