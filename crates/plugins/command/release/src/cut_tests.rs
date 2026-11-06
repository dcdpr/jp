use serde_json::{Value, json};

use super::*;
use crate::{options::Options, repository_testing::Fixture};

/// A highlight that follows the format.
const HIGHLIGHT: &str =
    "---\ntitle: Tool calls fan out\nauthors: [\"@JeanMertz\"]\n---\nThe assistant asks once.\n";

/// A `bump` that writes the version into `VERSION`, wherever git runs.
const BUMP: &str = "git config --file VERSION version.current {version}";

/// The options a test runs with: no forge, and [`BUMP`].
fn options() -> Options {
    options_with(&json!({ "forge": "none", "bump": BUMP }))
}

fn options_with(options: &Value) -> Options {
    let Value::Object(options) = options else {
        panic!("options are an object");
    };

    Options::read(options).unwrap()
}

fn version(text: &str) -> Version {
    Version::parse(text).unwrap()
}

/// The branches `origin` has.
fn branches(fixture: &Fixture) -> Vec<String> {
    fixture
        .origin_git(&["for-each-ref", "--format=%(refname:short)", "refs/heads/"])
        .lines()
        .map(ToOwned::to_owned)
        .collect()
}

#[test]
fn a_cut_starts_the_release_branch_and_files_the_notes_on_a_branch_of_their_own() {
    let fixture = Fixture::new();
    fixture.fragment("retry-idle-turns", "fixed", "Idle turns are retried.");
    fixture.write("changelog.d/highlights/fan-out.md", HIGHLIGHT);
    let main = fixture.commit("fix(llm): Retry idle turns (#1207)");
    fixture.push();
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options, None).unwrap();

    let said = cut(&pipeline, None).unwrap();

    assert_eq!(
        said,
        format!(
            "Started release 0.1.0 on release/0.1, cut from main at {}.\nPushed \
             release-cut/v0.1.0, which files 1 fragment and 1 highlight into \
             changelog.d/0.1.0/.\nMerge release-cut/v0.1.0 into main to file them.\n",
            &main[..7]
        )
    );

    // `main` at the cut, then the commit that starts the release: no
    // fragments, and the version written.
    assert_eq!(
        fixture.origin_git(&["rev-parse", "release/0.1^"]).trim(),
        main
    );
    assert_eq!(
        fixture.message("release/0.1").trim_end(),
        "chore(release): Start release 0.1.0\n\nStarts-Release: 0.1.0"
    );
    assert_eq!(fixture.show("release/0.1:changelog.d"), None);
    assert!(
        fixture
            .show("release/0.1:VERSION")
            .unwrap()
            .contains("current = 0.1.0")
    );

    // The notes, filed from the same commit.
    assert_eq!(
        fixture
            .origin_git(&["rev-parse", "release-cut/v0.1.0^"])
            .trim(),
        main
    );
    assert_eq!(
        fixture.show("release-cut/v0.1.0:changelog.d/0.1.0/retry-idle-turns.md"),
        Some("---\ntype: fixed\n---\nIdle turns are retried.\n".to_owned())
    );
    assert_eq!(
        fixture.show("release-cut/v0.1.0:changelog.d/0.1.0/highlights/fan-out.md"),
        Some(HIGHLIGHT.to_owned())
    );
    assert_eq!(
        fixture.show("release-cut/v0.1.0:changelog.d/retry-idle-turns.md"),
        None
    );
    assert!(
        fixture
            .show("release-cut/v0.1.0:changelog.d/README.md")
            .is_some()
    );
    assert!(
        fixture
            .show("release-cut/v0.1.0:VERSION")
            .unwrap()
            .contains("current = 0.1.0")
    );
    assert!(fixture.message("release-cut/v0.1.0").starts_with(
        "chore(release): Cut release 0.1.0\n\n`jp release cut` started release 0.1.0 on"
    ));

    // The checkout it ran from is left alone, and its worktrees are gone.
    assert_eq!(fixture.git(&["status", "--porcelain"]), "");
    assert_eq!(fixture.git(&["worktree", "list"]).lines().count(), 1);
}

/// JP tagged v0.1.0 before the pipeline existed.
#[test]
fn a_cut_after_a_tag_is_the_next_minor_release() {
    let fixture = Fixture::new();
    fixture.git(&["tag", "v0.1.0"]);
    fixture.git(&["push", "--quiet", "origin", "v0.1.0"]);
    fixture.fragment("retry", "fixed", "Retried.");
    fixture.commit("fix: Retry (#1)");
    fixture.push();
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options, None).unwrap();

    let said = cut(&pipeline, None).unwrap();

    assert!(
        said.starts_with("Started release 0.2.0 on release/0.2,"),
        "{said}"
    );
}

/// A fragment still at the top level because 0.2.0's change request hasn't
/// merged is shipped already, and left for that change request.
#[test]
fn a_fragment_another_release_ships_waits_for_its_change_request() {
    let fixture = Fixture::new();
    fixture.fragment("retry", "fixed", "Retried.");
    fixture.commit("fix: Retry (#1)");
    fixture.push();
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options, None).unwrap();
    cut(&pipeline, Some(&version("0.2.0"))).unwrap();
    fixture.fragment("model", "added", "Shows the model.");
    fixture.commit("feat: Show the model (#2)");
    fixture.push();

    let said = cut(&pipeline, None).unwrap();

    assert!(
        said.starts_with("Started release 0.3.0 on release/0.3,"),
        "{said}"
    );
    assert!(
        said.contains("which files 1 fragment into changelog.d/0.3.0/"),
        "{said}"
    );
    assert!(
        fixture
            .show("release-cut/v0.3.0:changelog.d/0.3.0/model.md")
            .is_some()
    );
    assert!(
        fixture
            .show("release-cut/v0.3.0:changelog.d/retry.md")
            .is_some()
    );
}

#[test]
fn from_1_0_on_a_breaking_change_cuts_a_major_release() {
    let fixture = Fixture::new();
    fixture.git(&["tag", "v1.2.0"]);
    fixture.git(&["push", "--quiet", "origin", "v1.2.0"]);
    fixture.write(
        "changelog.d/manifest.md",
        "---\ntype: changed\n---\nPlugins are found by their manifest.\n\nBREAKING CHANGE: \
         Plugins need a manifest\n\nRebuild them.\n",
    );
    fixture.commit("feat!: Route plugins by manifest (#3)");
    fixture.push();
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options, None).unwrap();

    let said = cut(&pipeline, None).unwrap();

    assert!(
        said.starts_with("Started release 2.0.0 on release/2.0,"),
        "{said}"
    );
}

/// A requested version is held to the same rule a picked one is: from 1.0 on,
/// only a major release ships a change that requires users to act.
#[test]
fn a_requested_minor_release_from_1_0_on_refuses_a_breaking_change() {
    let fixture = Fixture::new();
    fixture.git(&["tag", "v1.2.0"]);
    fixture.git(&["push", "--quiet", "origin", "v1.2.0"]);
    fixture.write(
        "changelog.d/manifest.md",
        "---\ntype: changed\n---\nPlugins are found by their manifest.\n\nBREAKING CHANGE: \
         Plugins need a manifest\n\nRebuild them.\n",
    );
    fixture.fragment("retry", "fixed", "Retried.");
    fixture.commit("feat!: Route plugins by manifest (#3)");
    fixture.push();
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options, None).unwrap();

    assert_eq!(
        cut(&pipeline, Some(&version("1.3.0"))),
        Err(
            "1.3.0 is a minor release from 1.0 on, which can't ship a change that requires users \
             to act, and changelog.d/manifest.md has an upgrade section. Leave out `--version` to \
             cut the next major release."
                .to_owned()
        )
    );
    assert_eq!(branches(&fixture), ["main"]);

    // A major release may.
    let said = cut(&pipeline, Some(&version("2.0.0"))).unwrap();
    assert!(
        said.starts_with("Started release 2.0.0 on release/2.0,"),
        "{said}"
    );
}

#[test]
fn a_main_without_unreleased_fragments_has_nothing_to_release() {
    let fixture = Fixture::new();
    fixture.write("changelog.d/highlights/fan-out.md", HIGHLIGHT);
    fixture.commit("Describe a feature");
    fixture.push();
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options, None).unwrap();

    assert_eq!(
        cut(&pipeline, None),
        Err("`main` has no unreleased fragments, so there is nothing to release.".to_owned())
    );
    assert_eq!(branches(&fixture), ["main"]);
}

#[test]
fn a_fragment_that_breaks_its_format_stops_the_cut() {
    let fixture = Fixture::new();
    fixture.write("changelog.d/notes.txt", "Remember the overview.\n");
    fixture.fragment("retry", "fixed", "Retried.");
    fixture.commit("fix: Retry (#1)");
    fixture.push();
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options, None).unwrap();

    assert_eq!(
        cut(&pipeline, None),
        Err("The fragments on `main` break their format; fix these \
             first:\nchangelog.d/notes.txt: not a fragment, which is named `<slug>.md`; rename \
             it, or move it out of the fragments directory"
            .to_owned())
    );
    assert_eq!(branches(&fixture), ["main"]);
}

/// The forge's needs are checked before anything is pushed.
#[test]
fn a_cut_that_cannot_open_its_change_request_pushes_nothing() {
    let fixture = Fixture::new();
    fixture.fragment("retry", "fixed", "Retried.");
    fixture.commit("fix: Retry (#1)");
    fixture.push();
    let shell = fixture.shell();
    let options = options_with(&json!({ "forge": "github" }));
    let pipeline = Pipeline::new(&shell, fixture.root(), &options, None).unwrap();

    let error = cut(&pipeline, None).unwrap_err();

    assert!(error.starts_with("The forge is `github`"), "{error}");
    assert_eq!(branches(&fixture), ["main"]);
}

/// A workspace below the top of its repository keeps its fragments, and runs
/// `bump`, there.
#[test]
fn a_workspace_below_the_top_of_its_repository_is_released_from_there() {
    let fixture = Fixture::new();
    fixture.write(
        "app/changelog.d/retry.md",
        "---\ntype: fixed\n---\nRetried.\n",
    );
    fixture.commit("fix: Retry (#1)");
    fixture.push();
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, &fixture.root().join("app"), &options, None).unwrap();

    cut(&pipeline, None).unwrap();

    assert!(
        fixture
            .show("release-cut/v0.1.0:app/changelog.d/0.1.0/retry.md")
            .is_some()
    );
    assert!(fixture.show("release-cut/v0.1.0:app/VERSION").is_some());
    assert_eq!(fixture.show("release/0.1:app/changelog.d"), None);
    // The fragments directory of another workspace in the repository stays.
    assert!(fixture.show("release/0.1:changelog.d/README.md").is_some());
}

#[test]
fn gitlab_is_asked_to_open_the_merge_request_through_the_push() {
    let fixture = Fixture::new();
    fixture.origin_git(&["config", "receive.advertisePushOptions", "true"]);
    fixture.fragment("retry", "fixed", "Retried.");
    fixture.commit("fix: Retry (#1)");
    fixture.push();
    let shell = fixture.shell();
    let options = options_with(&json!({
        "forge": "gitlab",
        "change_request_labels": ["no-changelog"],
    }));
    let pipeline = Pipeline::new(&shell, fixture.root(), &options, None).unwrap();

    let said = cut(&pipeline, None).unwrap();

    assert!(
        said.ends_with("Asked GitLab to open a merge request from release-cut/v0.1.0 into main.\n"),
        "{said}"
    );
    assert_eq!(branches(&fixture), [
        "main",
        "release-cut/v0.1.0",
        "release/0.1"
    ]);
}
