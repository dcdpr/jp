use std::collections::BTreeSet;

use camino_tempfile::Utf8TempDir;

use super::*;
use crate::{releases::Started, repository_testing::Fixture};

/// 2026-10-01 at noon UTC, in seconds since the Unix epoch.
const OCTOBER_1: i64 = 1_790_856_000;

fn version(text: &str) -> Version {
    Version::parse(text).unwrap()
}

/// Make a release branch for 0.3, here and on `origin`: the commit that starts
/// 0.3.0, and the commit `picked` picked onto it.
fn release_branch(fixture: &Fixture, picked: &str) -> String {
    fixture.git(&["checkout", "--quiet", "-b", "release/0.3"]);
    fixture.commit("Start release 0.3.0\n\nStarts-Release: 0.3.0");
    fixture.write("src/fix.rs", "// A fix.\n");
    let tip = fixture.commit(&format!(
        "Fix a bug\n\n(cherry picked from commit {picked})"
    ));
    fixture.publish(&["release/0.3"]);
    fixture.git(&["checkout", "--quiet", "main"]);

    tip
}

/// Commit to the branch `branch` without checking it out, and return the
/// commit's hash.
fn commit_on(fixture: &Fixture, branch: &str, message: &str) -> String {
    let tree = fixture.hash(&format!("{branch}^{{tree}}"));
    let parent = fixture.hash(branch);
    let commit = fixture
        .git(&["commit-tree", &tree, "-p", &parent, "-m", message])
        .trim()
        .to_owned();
    fixture.git(&["update-ref", &format!("refs/heads/{branch}"), &commit]);

    commit
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

/// A tag only this repository has finalizes its release too, but isn't
/// published.
#[test]
fn releases_are_read_from_the_tags_and_release_branches_here_and_on_origin() {
    let fixture = Fixture::new();
    let main = fixture.hash("HEAD");
    let tip = release_branch(&fixture, &main);
    fixture.git(&["tag", "--annotate", "v0.3.0", "--message", "v0.3.0", &tip]);
    fixture.publish(&["v0.3.0"]);
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
    assert_eq!(
        releases.tagged,
        BTreeSet::from([version("0.3.0"), version("0.4.0")])
    );
    assert_eq!(releases.published, BTreeSet::from([version("0.3.0")]));
    assert_eq!(releases.lines.len(), 1);
    assert_eq!(releases.lines[&(0, 3)], Line {
        branch: "release/0.3".to_owned(),
        tip,
        started: vec![Started {
            version: version("0.3.0"),
            hash: fixture.hash("release/0.3~1"),
            at: started_at,
        }],
        picked: BTreeSet::from([main.clone()]),
    });
    assert_eq!(repository.main(), Ok(main));
}

/// A release branch only this repository has, such as one a cut made and nobody
/// pushed, is read like any other.
#[test]
fn a_release_branch_origin_lacks_is_read_from_here() {
    let fixture = Fixture::new();
    let main = fixture.hash("HEAD");
    release_branch(&fixture, &main);
    let shell = fixture.shell();
    let repository = Repository::open(&shell, fixture.root()).unwrap();
    repository.fetch().unwrap();

    fixture.origin_git(&["branch", "--delete", "--force", "release/0.3"]);
    repository.fetch().unwrap();

    let branch = repository.branch("release/0.3").unwrap();
    assert_eq!(branch.origin, None);
    assert_eq!(repository.standing(&branch), Ok(Some(Standing::Local)));
    assert_eq!(repository.releases().unwrap().lines.len(), 1);
}

#[test]
fn a_branch_stands_against_origins_by_which_has_the_others_commits() {
    let fixture = Fixture::new();
    let main = fixture.hash("HEAD");
    release_branch(&fixture, &main);
    let shell = fixture.shell();
    let repository = Repository::open(&shell, fixture.root()).unwrap();
    repository.fetch().unwrap();
    let standing = || {
        let branch = repository.branch("release/0.3").unwrap();
        (
            repository.standing(&branch).unwrap(),
            repository.tip(&branch),
        )
    };
    let published = fixture.hash("release/0.3");

    assert_eq!(
        standing(),
        (Some(Standing::Published), Ok(Some(published.clone())))
    );

    let ahead = commit_on(&fixture, "release/0.3", "Pick another fix");
    assert_eq!(standing(), (Some(Standing::Ahead), Ok(Some(ahead))));

    fixture.git(&["update-ref", "refs/heads/release/0.3", &published]);
    fixture.publish(&["release/0.3"]);
    let behind = published.clone();
    let newer = commit_on(&fixture, "release/0.3", "Pick a fix elsewhere");
    fixture.publish(&["release/0.3"]);
    fixture.git(&["update-ref", "refs/heads/release/0.3", &behind]);
    repository.fetch().unwrap();
    assert_eq!(standing(), (Some(Standing::Behind), Ok(Some(newer))));

    commit_on(&fixture, "release/0.3", "Pick a fix here");
    assert_eq!(
        standing(),
        (
            Some(Standing::Diverged),
            Err(
                "release/0.3 and origin/release/0.3 have diverged; bring them together, such as \
                 with `git rebase origin/release/0.3 release/0.3`, and run the command again."
                    .to_owned()
            )
        )
    );

    let missing = repository.branch("release/0.9").unwrap();
    assert_eq!(repository.standing(&missing), Ok(None));
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

    // A release branch only this repository has holds its commits too.
    fixture.git(&["branch", "release/0.4", &later]);
    assert_eq!(
        repository.containing(&later),
        Ok(vec!["release/0.4".to_owned()])
    );
}

#[test]
fn the_checkout_stands_for_main_when_it_has_mains_commits() {
    let fixture = Fixture::new();
    let main = fixture.hash("HEAD");
    let shell = fixture.shell();
    let repository = Repository::open(&shell, fixture.root()).unwrap();
    repository.fetch().unwrap();

    let at_main = repository.checkout().unwrap();
    assert!(at_main.at_main());
    assert_eq!(at_main.describe(), format!("main at {}", &main[..7]));
    assert_eq!(at_main.ahead(), None);

    fixture.write("src/rehearsal.rs", "// A rehearsal.\n");
    let ahead = fixture.commit("A rehearsal");
    let checkout = repository.checkout().unwrap();
    assert_eq!(
        checkout.ahead(),
        Some(format!(
            "main at {} is ahead of origin's main at {}",
            &ahead[..7],
            &main[..7]
        ))
    );

    fixture.git(&["checkout", "--quiet", "--detach", &ahead]);
    assert_eq!(
        repository.checkout().unwrap().describe(),
        format!("the checkout at {}", &ahead[..7])
    );
}

#[test]
fn a_checkout_without_mains_commits_is_refused() {
    let fixture = Fixture::new();
    let behind = fixture.hash("HEAD");
    fixture.write("src/later.rs", "// Merged later.\n");
    let main = fixture.commit("A later change");
    fixture.push();
    fixture.git(&["reset", "--quiet", "--hard", &behind]);
    let shell = fixture.shell();
    let repository = Repository::open(&shell, fixture.root()).unwrap();
    repository.fetch().unwrap();

    assert_eq!(
        repository.checkout(),
        Err(format!(
            "main at {} lacks commits origin's main has, at {}; bring them in first, such as with \
             `git pull`.",
            &behind[..7],
            &main[..7]
        ))
    );
}

/// `git pull` on a branch other than `main` brings in that branch's upstream,
/// not `main`'s commits.
#[test]
fn a_branch_behind_main_is_told_to_merge_it() {
    let fixture = Fixture::new();
    let behind = fixture.hash("HEAD");
    fixture.write("src/later.rs", "// Merged later.\n");
    let main = fixture.commit("A later change");
    fixture.push();
    fixture.git(&["switch", "--quiet", "--create", "rehearsal", &behind]);
    let shell = fixture.shell();
    let repository = Repository::open(&shell, fixture.root()).unwrap();
    repository.fetch().unwrap();

    assert_eq!(
        repository.checkout(),
        Err(format!(
            "rehearsal at {} lacks commits origin's main has, at {}; bring them in first, such as \
             with `git merge origin/main`.",
            &behind[..7],
            &main[..7]
        ))
    );
}

#[test]
fn a_branch_is_set_only_from_where_it_was_read() {
    let fixture = Fixture::new();
    let first = fixture.hash("HEAD");
    fixture.write("src/next.rs", "// Next.\n");
    let second = fixture.commit("Next");
    let shell = fixture.shell();
    let repository = Repository::open(&shell, fixture.root()).unwrap();

    repository.set_branch("release/0.3", &first, None).unwrap();
    assert_eq!(fixture.hash("release/0.3"), first);

    // Created already, so it isn't created again.
    let error = repository
        .set_branch("release/0.3", &second, None)
        .unwrap_err();
    assert!(
        error.starts_with("release/0.3 moved while the command ran, so it wasn't changed: "),
        "{error}"
    );

    repository
        .set_branch("release/0.3", &second, Some(&first))
        .unwrap();
    assert_eq!(fixture.hash("release/0.3"), second);
}

#[test]
fn a_branch_checked_out_is_not_set() {
    let fixture = Fixture::new();
    let head = fixture.hash("HEAD");
    let shell = fixture.shell();
    let repository = Repository::open(&shell, fixture.root()).unwrap();

    let error = repository
        .set_branch("main", &head, Some(&head))
        .unwrap_err();

    assert!(error.starts_with("main is checked out at "), "{error}");
    assert!(
        error.ends_with("; switch that worktree to another branch and run the command again."),
        "{error}"
    );
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
    fixture.publish(&["v0.1.0", "v0.2.0"]);
    fixture.git(&["tag", "--delete", "v0.1.0", "v0.2.0"]);
    let shell = fixture.shell();
    let repository = Repository::open(&shell, fixture.root()).unwrap();

    assert_eq!(repository.tag_date("v0.1.0"), Ok(OCTOBER_1 + 3600));
    assert_eq!(repository.tag_date("v0.2.0"), Ok(OCTOBER_1));
    assert_eq!(fixture.git(&["tag", "--list"]), "");
}

/// A tag `finalize` made and nobody pushed is dated from this repository.
#[test]
fn a_tag_only_this_repository_has_is_dated_from_here() {
    let fixture = Fixture::new();
    fixture.git_at(OCTOBER_1, &[
        "tag",
        "--annotate",
        "v0.1.0",
        "--message",
        "Release 0.1.0",
    ]);
    let shell = fixture.shell();
    let repository = Repository::open(&shell, fixture.root()).unwrap();

    assert_eq!(repository.tag_date("v0.1.0"), Ok(OCTOBER_1));
}

/// An annotated tag names a tag object; both sides read the commit under it, as
/// they read a lightweight tag's.
#[test]
fn tags_name_their_commits_here_and_on_origin() {
    let fixture = Fixture::new();
    let commit = fixture.git(&["rev-parse", "HEAD"]).trim().to_owned();
    fixture.git(&["tag", "--annotate", "v0.1.0", "--message", "Release 0.1.0"]);
    fixture.git(&["tag", "v0.2.0"]);
    fixture.publish(&["v0.1.0"]);
    let shell = fixture.shell();
    let repository = Repository::open(&shell, fixture.root()).unwrap();

    let tags = repository.tags().unwrap();

    assert_eq!(
        tags.local,
        BTreeMap::from([
            ("v0.1.0".to_owned(), commit.clone()),
            ("v0.2.0".to_owned(), commit.clone()),
        ])
    );
    assert_eq!(tags.origin, BTreeMap::from([("v0.1.0".to_owned(), commit)]));
}

#[test]
fn a_tag_neither_has_cannot_be_dated() {
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
