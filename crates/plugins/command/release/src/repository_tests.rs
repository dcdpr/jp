use std::collections::BTreeSet;

use camino_tempfile::Utf8TempDir;

use super::*;
use crate::{releases::Started, repository_testing::Fixture};

/// 2026-10-01 at noon UTC, in seconds since the Unix epoch.
const OCTOBER_1: i64 = 1_790_856_000;

fn version(text: &str) -> Version {
    Version::parse(text).unwrap()
}

/// Push a release branch for 0.3 to `origin`: the commit that starts 0.3.0, and
/// the commit `picked` picked onto it.
fn release_branch(fixture: &Fixture, picked: &str) -> String {
    fixture.git(&["checkout", "--quiet", "-b", "release/0.3"]);
    fixture.commit("Start release 0.3.0\n\nStarts-Release: 0.3.0");
    fixture.write("src/fix.rs", "// A fix.\n");
    let tip = fixture.commit(&format!(
        "Fix a bug\n\n(cherry picked from commit {picked})"
    ));
    fixture.git(&["push", "--quiet", "origin", "release/0.3"]);
    fixture.git(&["checkout", "--quiet", "main"]);

    tip
}

#[test]
fn a_workspace_outside_a_repository_is_refused() {
    let shell = Shell::new();
    let dir = Utf8TempDir::new().unwrap();

    let Err(error) = Repository::open(&shell, dir.path()) else {
        panic!("expected a refusal");
    };

    assert!(
        error.starts_with(&format!(
            "The workspace at {} isn't in a git repository",
            dir.path()
        )),
        "{error}"
    );
}

#[test]
fn the_workspace_root_is_read_below_the_top_of_the_work_tree() {
    let fixture = Fixture::new();
    fixture.write("sub/project/README.md", "A workspace below the top.\n");
    let shell = fixture.shell();

    let top = Repository::open(&shell, fixture.root()).unwrap();
    let below = Repository::open(&shell, &fixture.root().join("sub/project")).unwrap();

    assert_eq!(top.prefix(), "");
    assert_eq!(below.prefix(), "sub/project/");
}

#[test]
fn releases_are_read_from_origins_tags_and_release_branches() {
    let fixture = Fixture::new();
    let main = fixture.hash("HEAD");
    let tip = release_branch(&fixture, &main);
    fixture.git(&["tag", "--annotate", "v0.3.0", "--message", "v0.3.0", &tip]);
    fixture.git(&["push", "--quiet", "origin", "v0.3.0"]);
    // A tag only the workspace has finalizes nothing.
    fixture.git(&["tag", "--annotate", "v0.4.0", "--message", "v0.4.0"]);
    let shell = fixture.shell();
    let repository = Repository::open(&shell, fixture.root()).unwrap();

    repository.fetch().unwrap();
    let releases = repository.releases().unwrap();

    // The commit that starts 0.3.0 is the one below the tip.
    let started_at = fixture
        .git(&["log", "-1", "--format=%ct", "release/0.3~1"])
        .trim()
        .parse()
        .unwrap();
    assert_eq!(releases.tagged, BTreeSet::from([version("0.3.0")]));
    assert_eq!(releases.lines.len(), 1);
    assert_eq!(releases.lines[&(0, 3)], Line {
        branch: "release/0.3".to_owned(),
        tip,
        started: vec![Started {
            version: version("0.3.0"),
            at: started_at,
        }],
        picked: BTreeSet::from([main.clone()]),
    });
    assert_eq!(repository.main(), Ok(main));
}

/// A release branch `origin` dropped, such as an abandoned release's, ships
/// nothing.
#[test]
fn a_fetch_drops_the_release_branches_origin_no_longer_has() {
    let fixture = Fixture::new();
    let main = fixture.hash("HEAD");
    release_branch(&fixture, &main);
    let shell = fixture.shell();
    let repository = Repository::open(&shell, fixture.root()).unwrap();
    repository.fetch().unwrap();

    fixture.origin_git(&["branch", "--delete", "--force", "release/0.3"]);
    repository.fetch().unwrap();

    assert_eq!(repository.releases().unwrap().lines.len(), 0);
}

#[test]
fn the_release_branches_holding_a_commit_are_listed() {
    let fixture = Fixture::new();
    let main = fixture.hash("HEAD");
    release_branch(&fixture, &main);
    fixture.write("src/later.rs", "// Merged after the cut.\n");
    let later = fixture.commit("A later change");
    fixture.push();
    let shell = fixture.shell();
    let repository = Repository::open(&shell, fixture.root()).unwrap();
    repository.fetch().unwrap();

    assert_eq!(
        repository.containing(&main),
        Ok(vec!["release/0.3".to_owned()])
    );
    assert_eq!(repository.containing(&later), Ok(vec![]));
}

#[test]
fn a_branch_is_looked_up_on_origin() {
    let fixture = Fixture::new();
    let shell = fixture.shell();
    let repository = Repository::open(&shell, fixture.root()).unwrap();

    assert_eq!(repository.has_branch("main"), Ok(true));
    assert_eq!(repository.has_branch("release-cut/v0.3.0"), Ok(false));
}

#[test]
fn a_commit_is_named_by_any_revision() {
    let fixture = Fixture::new();
    let head = fixture.hash("HEAD");
    let shell = fixture.shell();
    let repository = Repository::open(&shell, fixture.root()).unwrap();

    assert_eq!(repository.commit(&head[..7]), Ok(head));
    assert_eq!(
        repository.commit("no-such-branch"),
        Err("`no-such-branch` names no commit.".to_owned())
    );
}

#[test]
fn a_worktree_is_removed_when_dropped_unless_kept() {
    let fixture = Fixture::new();
    let head = fixture.hash("HEAD");
    let shell = fixture.shell();
    let repository = Repository::open(&shell, fixture.root()).unwrap();

    let worktree = repository.worktree("test", &head).unwrap();
    let top = worktree.top().to_owned();
    assert_eq!(worktree.head(), Ok(head.clone()));
    assert!(top.join("changelog.d/README.md").exists());
    drop(worktree);

    assert!(!top.exists());
    assert!(!fixture.git(&["worktree", "list"]).contains(top.as_str()));

    let kept = repository.worktree("test", &head).unwrap().keep();
    assert!(kept.exists());
    drop(repository.reopen(&kept).unwrap());
    assert!(!kept.exists());
}

/// Pushing a new branch refuses to replace one another run made meanwhile.
#[test]
fn a_push_that_creates_a_branch_refuses_to_replace_one() {
    let fixture = Fixture::new();
    let head = fixture.hash("HEAD");
    let shell = fixture.shell();
    let repository = Repository::open(&shell, fixture.root()).unwrap();

    repository.push(&head, "release/0.3", true, &[]).unwrap();
    fixture.write("src/other.rs", "// Another change.\n");
    let other = fixture.commit("Another change");
    let error = repository
        .push(&other, "release/0.3", true, &[])
        .unwrap_err();

    assert!(
        error.starts_with("Pushing release/0.3 to origin failed: "),
        "{error}"
    );
    assert_eq!(
        fixture.origin_git(&["rev-parse", "release/0.3"]).trim(),
        head
    );
}

/// A release tagged by hand, without a release branch, is dated by its tag,
/// read from `origin` without writing a tag to the repository.
#[test]
fn a_tag_is_dated_by_its_tagger_or_else_by_its_commit() {
    let fixture = Fixture::new();
    fixture.git_at(OCTOBER_1, &[
        "commit",
        "--quiet",
        "--allow-empty",
        "--message",
        "Ship it",
    ]);
    fixture.git_at(OCTOBER_1 + 3600, &[
        "tag",
        "--annotate",
        "v0.1.0",
        "--message",
        "Release 0.1.0",
    ]);
    fixture.git(&["tag", "v0.2.0"]);
    fixture.git(&["push", "--quiet", "origin", "v0.1.0", "v0.2.0"]);
    fixture.git(&["tag", "--delete", "v0.1.0", "v0.2.0"]);
    let shell = fixture.shell();
    let repository = Repository::open(&shell, fixture.root()).unwrap();

    assert_eq!(repository.tag_date("v0.1.0"), Ok(OCTOBER_1 + 3600));
    assert_eq!(repository.tag_date("v0.2.0"), Ok(OCTOBER_1));
    assert_eq!(fixture.git(&["tag", "--list"]), "");
}

#[test]
fn a_tag_origin_does_not_have_cannot_be_dated() {
    let fixture = Fixture::new();
    let shell = fixture.shell();
    let repository = Repository::open(&shell, fixture.root()).unwrap();

    let error = repository.tag_date("v0.9.0").unwrap_err();

    assert!(error.starts_with("`git fetch` failed: "), "{error}");
}

#[test]
fn a_shallow_clone_is_refused() {
    let fixture = Fixture::new();
    let clone = Utf8TempDir::new().unwrap();
    // Only a `file://` address clones shallow, and its path has to start with
    // `/`, which a Windows path does not.
    let path = fixture.origin().as_str().replace('\\', "/");
    let address = format!("file:///{}", path.trim_start_matches('/'));
    fixture.git(&[
        "clone",
        "--quiet",
        "--depth=1",
        &address,
        clone.path().as_str(),
    ]);
    let shell = fixture.shell();
    let repository = Repository::open(&shell, clone.path()).unwrap();

    assert!(repository.refuse_shallow().is_err());
}
