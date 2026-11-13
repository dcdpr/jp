use super::*;

const HASH: &str = "4e1f0a2c9d8b7a6f5e4d3c2b1a0f9e8d7c6b5a49";

fn number(n: u64) -> NonZeroU64 {
    NonZeroU64::new(n).unwrap()
}

#[test]
fn forges_are_named_as_configuration_names_them() {
    assert_eq!("github".parse::<Forge>(), Ok(Forge::GitHub));
    assert_eq!("gitlab".parse::<Forge>(), Ok(Forge::GitLab));
    assert_eq!("none".parse::<Forge>(), Ok(Forge::None));

    assert_eq!(
        "GitHub".parse::<Forge>().unwrap_err().to_string(),
        "`GitHub` is not a forge; use `github`, `gitlab`, or `none`"
    );
}

#[test]
fn the_forge_is_inferred_from_the_host_of_the_projects_address() {
    assert_eq!(
        Forge::infer(Some("https://github.com/dcdpr/jp")),
        Forge::GitHub
    );
    assert_eq!(
        Forge::infer(Some("https://GitLab.com/group/sub/project")),
        Forge::GitLab
    );

    // A host of GitHub's own looks like any other host.
    assert_eq!(
        Forge::infer(Some("https://github.example.com/dcdpr/jp")),
        Forge::None
    );
    assert_eq!(Forge::infer(Some("https://example.com/jp")), Forge::None);
    assert_eq!(Forge::infer(None), Forge::None);
}

#[test]
fn a_github_squash_commit_names_its_pull_request_at_the_end_of_its_subject() {
    let forge = Forge::GitHub;

    assert_eq!(
        forge.change_request("feat(cli): Fan tool calls out (#1182)\n\nBody.\n"),
        Some(number(1182))
    );
    // A title that already ends in a reference gets GitHub's after it.
    assert_eq!(
        forge.change_request("Revert \"Fix retries (#45)\" (#1207)"),
        Some(number(1207))
    );

    assert_eq!(forge.change_request("Fix retries\n\nCloses (#45)\n"), None);
    assert_eq!(forge.change_request("Fix (#12 and #13)"), None);
    assert_eq!(forge.change_request("Fix (#0)"), None);
}

#[test]
fn a_github_merge_commit_names_its_pull_request_at_the_start_of_its_subject() {
    let forge = Forge::GitHub;

    assert_eq!(
        forge.change_request("Merge pull request #1216 from JeanMertz/manifests\n\nTitle\n"),
        Some(number(1216))
    );
    assert_eq!(forge.change_request("Merge branch 'feature'"), None);
}

#[test]
fn a_gitlab_merge_commit_names_its_merge_request_on_its_last_line() {
    let forge = Forge::GitLab;
    let message = "Merge branch 'retry' into 'main'\n\nRetry idle turns\n\nSee merge request \
                   group/sub/project!42\n";

    assert_eq!(forge.change_request(message), Some(number(42)));
    // GitLab's default squash commit is the merge request's title alone.
    assert_eq!(forge.change_request("Retry idle turns (#42)"), None);
}

#[test]
fn without_a_forge_no_commit_names_a_change_request() {
    assert_eq!(
        Forge::None.change_request("Fan tool calls out (#1182)"),
        None
    );
}

#[test]
fn github_credits_link_to_pull_requests_and_commits() {
    let project = Project::new(Forge::GitHub, Some("https://github.com/dcdpr/jp/")).unwrap();

    assert_eq!(project.credit(HASH, "Fan tool calls out (#1182)"), Credit {
        kind: CreditKind::ChangeRequest,
        label: "#1182".to_owned(),
        url: Some("https://github.com/dcdpr/jp/pull/1182".to_owned()),
    });
    assert_eq!(project.credit(HASH, "Fix the help text"), Credit {
        kind: CreditKind::Commit,
        label: "4e1f0a2".to_owned(),
        url: Some(format!("https://github.com/dcdpr/jp/commit/{HASH}")),
    });
}

#[test]
fn gitlab_credits_link_to_merge_requests_and_commits() {
    let project = Project::new(Forge::GitLab, Some("https://gitlab.com/group/project")).unwrap();

    assert_eq!(project.change_request(number(42)), Credit {
        kind: CreditKind::ChangeRequest,
        label: "!42".to_owned(),
        url: Some("https://gitlab.com/group/project/-/merge_requests/42".to_owned()),
    });
    assert_eq!(
        project.commit(HASH).url,
        Some(format!("https://gitlab.com/group/project/-/commit/{HASH}"))
    );
}

#[test]
fn without_a_forge_credits_link_nowhere() {
    let project = Project::new(Forge::None, Some("https://example.com/jp")).unwrap();

    assert_eq!(project.credit(HASH, "Fan tool calls out (#1182)"), Credit {
        kind: CreditKind::Commit,
        label: "4e1f0a2".to_owned(),
        url: None,
    });
    // A change request a fragment names itself.
    assert_eq!(project.change_request(number(7)), Credit {
        kind: CreditKind::ChangeRequest,
        label: "#7".to_owned(),
        url: None,
    });
}

#[test]
fn a_forge_that_links_needs_an_address() {
    assert_eq!(
        Project::new(Forge::GitHub, None),
        Err(NoAddress(Forge::GitHub))
    );
    assert_eq!(
        NoAddress(Forge::GitLab).to_string(),
        "the forge is `gitlab`, but git's `origin` remote has no web address for credits to link \
         to"
    );
    assert!(Project::new(Forge::None, None).is_ok());
}
