use std::slice;

use serde_json::{Value, json};

use super::*;
use crate::{
    cut::cut, finalize::finalize, options::Options, pick::pick, repository_testing::Fixture,
    shell::Shell,
};

/// 2026-10-01 at noon UTC, in seconds since the Unix epoch.
const OCTOBER_1: i64 = 1_790_856_000;

/// A day, in seconds.
const DAY: i64 = 86_400;

fn options() -> Options {
    let Value::Object(options) = json!({ "forge": "none" }) else {
        panic!("options are an object");
    };

    Options::read(&options).unwrap()
}

/// What `status` says at `now` in the workspace of `fixture`.
fn status_at(fixture: &Fixture, now: i64) -> String {
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options).unwrap();

    status(&pipeline, now).unwrap()
}

/// Run `command` in the workspace of `fixture` with `shell`.
fn run(
    fixture: &Fixture,
    shell: &Shell,
    command: impl FnOnce(&Pipeline<'_>) -> Result<String, String>,
) -> Result<String, String> {
    let options = options();
    let pipeline = Pipeline::new(shell, fixture.root(), &options).unwrap();

    command(&pipeline)
}

/// A workspace with a fix merged into `main`, and release 0.1.0 cut from it
/// here on 2026-10-01, its branches not yet published.
fn cut_on_october_1() -> Fixture {
    let fixture = Fixture::new();
    fixture.fragment("retry", "fixed", "Retried.");
    fixture.commit("fix: Retry (#1)");
    fixture.push();
    run(&fixture, &fixture.shell_at(OCTOBER_1), |pipeline| {
        cut(pipeline, None, None)
    })
    .unwrap();

    fixture
}

#[test]
fn before_any_release_the_first_is_due_once_a_fragment_is_merged() {
    let fixture = Fixture::new();
    assert_eq!(
        status_at(&fixture, OCTOBER_1),
        "`main` has no unreleased fragments, so there is nothing to release yet; the first \
         release is due once a fragment is merged.\n"
    );

    fixture.fragment("retry", "fixed", "Retried.");
    fixture.commit("fix: Retry (#1)");
    fixture.push();

    assert_eq!(
        status_at(&fixture, OCTOBER_1),
        "No release has been cut yet, so the first one is due.\n\nCut it with:\n\n    jp release \
         cut\n"
    );
}

#[test]
fn a_cut_not_yet_published_says_how_to_publish_it_and_what_comes_next() {
    let fixture = cut_on_october_1();
    let head = fixture.hash("HEAD");

    assert_eq!(
        status_at(&fixture, OCTOBER_1),
        format!(
            "Only this repository has release/0.1 and release-notes/v0.1.0. Publish them \
             with:\n\n    git push origin release/0.1 release-notes/v0.1.0\n\nor, if a rehearsal \
             left them, delete them with `git branch -D release/0.1 \
             release-notes/v0.1.0`.\n\n0.1.0 is in preparation on release/0.1. Its notes are on \
             release-notes/v0.1.0, and main at {} lacks some of them: merge release-notes/v0.1.0 \
             into main once they are final, then finalize it.\n",
            &head[..7]
        )
    );
}

#[test]
fn merged_notes_say_to_finalize() {
    let fixture = cut_on_october_1();
    fixture.publish(&["release/0.1", "release-notes/v0.1.0"]);
    fixture.merge_notes("0.1.0", "chore(release): Cut release 0.1.0 (#2)");
    let head = fixture.hash("HEAD");

    assert_eq!(
        status_at(&fixture, OCTOBER_1),
        format!(
            "0.1.0 is in preparation on release/0.1. Its notes are on main at {}. Once \
             release/0.1's checks pass, finalize it with:\n\n    jp release finalize 0.1.0\n",
            &head[..7]
        )
    );
}

/// A notes branch merged and then deleted on `origin`, as a forge does once a
/// pull request merges, stays here; it is no reason to push.
#[test]
fn a_merged_notes_branch_left_here_is_not_pushed_again() {
    let fixture = cut_on_october_1();
    fixture.publish(&["release/0.1", "release-notes/v0.1.0"]);
    fixture.merge_notes("0.1.0", "chore(release): Cut release 0.1.0 (#2)");
    fixture.git(&[
        "push",
        "--quiet",
        "origin",
        "--delete",
        "release-notes/v0.1.0",
    ]);

    assert!(
        !status_at(&fixture, OCTOBER_1).contains("Only this repository has"),
        "{}",
        status_at(&fixture, OCTOBER_1)
    );
}

#[test]
fn a_release_tagged_here_says_how_to_publish_it_and_when_the_next_is_due() {
    let fixture = cut_on_october_1();
    fixture.publish(&["release/0.1", "release-notes/v0.1.0"]);
    fixture.merge_notes("0.1.0", "chore(release): Cut release 0.1.0 (#2)");
    run(&fixture, &fixture.shell(), |pipeline| {
        finalize(pipeline, &Version::parse("0.1.0").unwrap())
    })
    .unwrap();

    assert_eq!(
        status_at(&fixture, OCTOBER_1 + DAY),
        "Only this repository has v0.1.0. Publish it with:\n\n    git push origin v0.1.0\n\nor, \
         if a rehearsal left it, delete it with `git tag -d v0.1.0`.\n\n0.1.0 was cut on \
         2026-10-01, and the cadence is 2w, so the next release is due at 2026-10-15 12:00 UTC.\n"
    );
}

#[test]
fn a_stopped_pick_says_how_to_continue() {
    let fixture = cut_on_october_1();
    fixture.write("src/lib.rs", "two\n");
    fixture.commit("Change the library");
    fixture.write("src/lib.rs", "three\n");
    fixture.fragment("three", "fixed", "Three.");
    let hash = fixture.commit("fix: Change the library again (#10)");
    fixture.push();
    run(&fixture, &fixture.shell(), |pipeline| {
        pick(
            pipeline,
            &Version::parse("0.1.0").unwrap(),
            slice::from_ref(&hash),
        )
    })
    .unwrap_err();
    let (version, worktree) = {
        let shell = fixture.shell();
        let options = options();
        let pipeline = Pipeline::new(&shell, fixture.root(), &options).unwrap();
        pick::stopped(&pipeline).unwrap().unwrap()
    };

    assert_eq!(version, "0.1.0");
    assert!(status_at(&fixture, OCTOBER_1).starts_with(&format!(
            "A pick into 0.1.0 stopped at a conflict in {worktree}; resolve it there, then \
             run:\n\n    jp release pick --continue\n\n"
        )));
}

/// `cut` and `finalize` refuse a checkout behind `origin`'s `main`, so that
/// comes first.
#[test]
fn a_checkout_behind_main_says_to_bring_it_in_first() {
    let fixture = Fixture::new();
    let start = fixture.hash("HEAD");
    fixture.fragment("retry", "fixed", "Retried.");
    let fix = fixture.commit("fix: Retry (#1)");
    fixture.push();
    fixture.git(&["reset", "--quiet", "--hard", &start]);

    assert!(status_at(&fixture, OCTOBER_1).starts_with(&format!(
        "main at {} lacks commits origin's main has, at {}; bring them in first, such as with \
         `git pull`.\n\n",
        &start[..7],
        &fix[..7]
    )));
}

/// A notes branch edited here and elsewhere has to be brought together before
/// it can be merged with both.
#[test]
fn a_diverged_notes_branch_is_named() {
    let fixture = cut_on_october_1();
    fixture.publish(&["release/0.1", "release-notes/v0.1.0"]);
    fixture.git(&["switch", "--quiet", "release-notes/v0.1.0"]);
    fixture.git(&["reset", "--quiet", "--hard", "HEAD~1"]);
    fixture.write("changelog.d/0.1.0/overview.md", "An overview.\n");
    fixture.commit("Write the overview here");
    fixture.git(&["switch", "--quiet", "main"]);

    let said = status_at(&fixture, OCTOBER_1);

    assert!(
        said.contains(
            "release-notes/v0.1.0 here and on origin each have commits the other lacks; bring it \
             together before the next release command."
        ),
        "{said}"
    );
}
