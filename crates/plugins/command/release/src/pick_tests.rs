use std::slice;

use serde_json::{Value, json};

use super::*;
use crate::{cut::cut, finalize::finalize, options::Options, repository_testing::Fixture};

/// A `bump` that writes the version into `VERSION`, wherever git runs.
const BUMP: &str = "git config --file VERSION version.current {version}";

fn options() -> Options {
    options_for("none")
}

/// The options of a project on `forge`.
fn options_for(forge: &str) -> Options {
    let Value::Object(options) = json!({ "forge": forge, "bump": BUMP }) else {
        panic!("options are an object");
    };

    Options::read(&options).unwrap()
}

fn version(text: &str) -> Version {
    Version::parse(text).unwrap()
}

/// A workspace whose release 0.1.0 was cut from a fix, its notes' change
/// request left open.
fn cut_0_1_0() -> Fixture {
    let fixture = Fixture::new();
    fixture.fragment("retry", "fixed", "Retried.");
    fixture.commit("fix: Retry (#1)");
    fixture.push();
    run(&fixture, |pipeline| cut(pipeline, None)).unwrap();

    fixture
}

/// Run `command` in the workspace of `fixture`, without a forge.
fn run(
    fixture: &Fixture,
    command: impl FnOnce(&Pipeline<'_>) -> Result<String, String>,
) -> Result<String, String> {
    run_with(fixture, &options(), command)
}

/// Run `command` in the workspace of `fixture` with `options`.
fn run_with(
    fixture: &Fixture,
    options: &Options,
    command: impl FnOnce(&Pipeline<'_>) -> Result<String, String>,
) -> Result<String, String> {
    let shell = fixture.shell();
    let pipeline = Pipeline::new(&shell, fixture.root(), options, None).unwrap();

    command(&pipeline)
}

/// A workspace on GitLab whose release 0.1.0 is tagged, and the hash of a fix
/// merged since through merge request `!5`.
///
/// `origin` takes push options, as GitLab does.
fn on_gitlab_with_a_fix() -> (Fixture, String) {
    let fixture = cut_0_1_0();
    fixture.origin_git(&["config", "receive.advertisePushOptions", "true"]);
    run(&fixture, |pipeline| finalize(pipeline, &version("0.1.0"))).unwrap();
    fixture.write("src/fix.rs", "// A fix.\n");
    fixture.fragment("fix", "fixed", "A fix.");
    let hash =
        fixture.commit("Merge branch 'fix' into 'main'\n\nSee merge request group/project!5");
    fixture.push();

    (fixture, hash)
}

/// Commit a change to `path` and a fragment `slug` with `message`, push it, and
/// return the commit's hash.
fn change(fixture: &Fixture, path: &str, slug: &str, message: &str) -> String {
    fixture.write(path, &format!("// {message}\n"));
    fixture.fragment(slug, "fixed", message);
    let hash = fixture.commit(message);
    fixture.push();

    hash
}

/// The branches `origin` has.
fn branches(fixture: &Fixture) -> Vec<String> {
    fixture
        .origin_git(&["for-each-ref", "--format=%(refname:short)", "refs/heads/"])
        .lines()
        .map(ToOwned::to_owned)
        .collect()
}

/// The worktree a stopped pick kept.
fn stopped(fixture: &Fixture) -> Utf8PathBuf {
    let state = fs::read_to_string(fixture.root().join(".git").join(STOPPED)).unwrap();
    let stopped: Stopped = serde_json::from_str(&state).unwrap();

    stopped.worktree
}

#[test]
fn a_pick_into_the_release_in_preparation_moves_its_fragment() {
    let fixture = cut_0_1_0();
    let hash = change(
        &fixture,
        "src/model.rs",
        "model",
        "feat: Show the model (#2)",
    );
    let short = &hash[..7];

    let said = run(&fixture, |pipeline| {
        pick(pipeline, &version("0.1.0"), &[short.to_owned()])
    })
    .unwrap();

    assert_eq!(
        said,
        format!(
            "Picked 1 commit onto release/0.1 for release 0.1.0, and pushed it.\nPushed \
             release-pick/v0.1.0-{short}, which files 1 fragment into changelog.d/0.1.0/.\nMerge \
             release-pick/v0.1.0-{short} into main to file them.\n"
        )
    );

    // The code, picked without its fragment.
    assert!(
        fixture
            .message("release/0.1")
            .contains(&format!("(cherry picked from commit {hash})"))
    );
    assert!(fixture.show("release/0.1:src/model.rs").is_some());
    assert_eq!(fixture.show("release/0.1:changelog.d"), None);

    // The fragment, moved on a branch from `main`.
    let branch = format!("release-pick/v0.1.0-{short}");
    assert_eq!(
        fixture
            .origin_git(&["rev-parse", &format!("{branch}^")])
            .trim(),
        hash
    );
    assert_eq!(
        fixture.show(&format!("{branch}:changelog.d/0.1.0/model.md")),
        Some("---\ntype: fixed\n---\nfeat: Show the model (#2)\n".to_owned())
    );
    assert_eq!(
        fixture.show(&format!("{branch}:changelog.d/model.md")),
        None
    );
    assert_eq!(fixture.git(&["worktree", "list"]).lines().count(), 1);
}

/// The fragment of a picked commit waits for the pick's change request, so the
/// next cut doesn't list the change again.
#[test]
fn the_next_cut_leaves_a_fragment_a_pick_ships() {
    let fixture = cut_0_1_0();
    let picked = change(
        &fixture,
        "src/model.rs",
        "model",
        "feat: Show the model (#2)",
    );
    run(&fixture, |pipeline| {
        pick(pipeline, &version("0.1.0"), slice::from_ref(&picked))
    })
    .unwrap();
    change(&fixture, "src/later.rs", "later", "fix: A later fix (#3)");

    run(&fixture, |pipeline| cut(pipeline, None)).unwrap();

    assert!(
        fixture
            .show("release-cut/v0.2.0:changelog.d/0.2.0/later.md")
            .is_some()
    );
    assert!(
        fixture
            .show("release-cut/v0.2.0:changelog.d/model.md")
            .is_some()
    );
}

#[test]
fn a_pick_after_the_last_release_was_tagged_starts_the_next_patch_release() {
    let fixture = cut_0_1_0();
    run(&fixture, |pipeline| finalize(pipeline, &version("0.1.0"))).unwrap();
    let hash = change(&fixture, "src/fix.rs", "fix", "fix: A fix (#3)");

    let said = run(&fixture, |pipeline| {
        pick(pipeline, &version("0.1.1"), slice::from_ref(&hash))
    })
    .unwrap();

    assert!(
        said.starts_with(
            "Started release 0.1.1 on release/0.1 with 1 commit picked onto it, and pushed it.\n"
        ),
        "{said}"
    );
    assert_eq!(
        fixture.message("release/0.1~1").trim_end(),
        "chore(release): Start release 0.1.1\n\nStarts-Release: 0.1.1"
    );
    assert!(
        fixture
            .show("release/0.1~1:VERSION")
            .unwrap()
            .contains("current = 0.1.1")
    );
    assert!(
        fixture
            .show(&format!(
                "release-pick/v0.1.1-{}:changelog.d/0.1.1/fix.md",
                &hash[..7]
            ))
            .is_some()
    );
}

/// 0.2.0 was cut after the fix merged, so both releases ship it and both list
/// it; the copy keeps the credits history gives the original.
#[test]
fn a_fragment_a_later_release_ships_too_is_copied_with_its_credits() {
    let (fixture, hash) = on_gitlab_with_a_fix();
    let gitlab = options_for("gitlab");
    run_with(&fixture, &gitlab, |pipeline| cut(pipeline, None)).unwrap();

    let said = run_with(&fixture, &gitlab, |pipeline| {
        pick(pipeline, &version("0.1.1"), slice::from_ref(&hash))
    })
    .unwrap();

    assert!(
        said.ends_with(&format!(
            "Asked GitLab to open a merge request from release-pick/v0.1.1-{} into main.\n",
            &hash[..7]
        )),
        "{said}"
    );
    let branch = format!("release-pick/v0.1.1-{}", &hash[..7]);
    assert_eq!(
        fixture.show(&format!("{branch}:changelog.d/0.1.1/fix.md")),
        Some("---\ntype: fixed\nchange_requests: [5]\n---\nA fix.\n".to_owned())
    );
    // Left where it was, for 0.2.0's change request to move.
    assert!(
        fixture
            .show(&format!("{branch}:changelog.d/fix.md"))
            .is_some()
    );
}

/// Once 0.2.0's notes are filed on `main`, the copy is made from its directory,
/// with the credits history followed back to the top level.
#[test]
fn a_fragment_filed_into_a_later_release_is_copied_from_its_directory() {
    let (fixture, hash) = on_gitlab_with_a_fix();
    let gitlab = options_for("gitlab");
    run_with(&fixture, &gitlab, |pipeline| cut(pipeline, None)).unwrap();
    fixture.git(&[
        "pull",
        "--quiet",
        "--ff-only",
        "origin",
        "release-cut/v0.2.0",
    ]);
    fixture.push();

    run_with(&fixture, &gitlab, |pipeline| {
        pick(pipeline, &version("0.1.1"), slice::from_ref(&hash))
    })
    .unwrap();

    let branch = format!("release-pick/v0.1.1-{}", &hash[..7]);
    assert_eq!(
        fixture.show(&format!("{branch}:changelog.d/0.1.1/fix.md")),
        Some("---\ntype: fixed\nchange_requests: [5]\n---\nA fix.\n".to_owned())
    );
    assert!(
        fixture
            .show(&format!("{branch}:changelog.d/0.2.0/fix.md"))
            .is_some()
    );
}

#[test]
fn a_patch_release_refuses_a_fragment_with_an_upgrade_section() {
    let fixture = cut_0_1_0();
    run(&fixture, |pipeline| finalize(pipeline, &version("0.1.0"))).unwrap();
    let tip = fixture.origin_git(&["rev-parse", "release/0.1"]);
    fixture.write("src/plugins.rs", "// Plugins need a manifest.\n");
    fixture.write(
        "changelog.d/manifest.md",
        "---\ntype: changed\n---\nPlugins are found by their manifest.\n\nBREAKING CHANGE: \
         Plugins need a manifest\n\nRebuild them.\n",
    );
    let hash = fixture.commit("feat!: Route plugins by manifest (#4)");
    fixture.push();

    let error = run(&fixture, |pipeline| {
        pick(pipeline, &version("0.1.1"), slice::from_ref(&hash))
    })
    .unwrap_err();

    assert_eq!(
        error,
        format!(
            "0.1.1 is a patch release, which can't ship a change that requires users to act, and \
             {} added changelog.d/manifest.md, which has an upgrade section.",
            &hash[..7]
        )
    );
    assert_eq!(fixture.origin_git(&["rev-parse", "release/0.1"]), tip);
    assert_eq!(branches(&fixture), [
        "main",
        "release-cut/v0.1.0",
        "release/0.1"
    ]);
}

#[test]
fn a_commit_the_release_branch_has_is_refused() {
    let fixture = cut_0_1_0();
    let cut_at = fixture.hash("main");

    let error = run(&fixture, |pipeline| {
        pick(pipeline, &version("0.1.0"), slice::from_ref(&cut_at))
    })
    .unwrap_err();

    assert_eq!(error, format!("release/0.1 has {} already.", &cut_at[..7]));
}

#[test]
fn a_commit_off_mains_first_parent_line_is_refused() {
    let fixture = cut_0_1_0();
    fixture.git(&["checkout", "--quiet", "-b", "feature"]);
    fixture.write("src/feature.rs", "// A feature.\n");
    let feature = fixture.commit("A feature");
    fixture.git(&["checkout", "--quiet", "main"]);

    let error = run(&fixture, |pipeline| {
        pick(pipeline, &version("0.1.0"), slice::from_ref(&feature))
    })
    .unwrap_err();

    assert!(
        error.starts_with(&format!("`{feature}` isn't on `main`'s first-parent line")),
        "{error}"
    );
}

#[test]
fn a_commit_that_only_changes_notes_is_refused() {
    let fixture = cut_0_1_0();
    fixture.fragment("retry", "fixed", "Retried, with a correction.");
    let hash = fixture.commit("docs: Correct a fragment (#8)");
    fixture.push();

    let error = run(&fixture, |pipeline| {
        pick(pipeline, &version("0.1.0"), slice::from_ref(&hash))
    })
    .unwrap_err();

    assert_eq!(
        error,
        format!(
            "{} changes nothing outside changelog.d, and release notes are corrected on `main`, \
             not picked.",
            &hash[..7]
        )
    );
}

/// A correction to a fragment conflicts on a release branch, which has no
/// fragments; leaving the fragment out resolves it.
#[test]
fn a_conflict_in_notes_alone_is_resolved_by_leaving_them_out() {
    let fixture = cut_0_1_0();
    fixture.write("src/retry.rs", "// Retry longer.\n");
    fixture.fragment("retry", "fixed", "Retried, for longer.");
    let hash = fixture.commit("fix: Retry for longer (#9)");
    fixture.push();

    let said = run(&fixture, |pipeline| {
        pick(pipeline, &version("0.1.0"), slice::from_ref(&hash))
    })
    .unwrap();

    assert_eq!(
        said,
        "Picked 1 commit onto release/0.1 for release 0.1.0, and pushed it.\nThe commit added no \
         fragment or highlight to file on main.\n"
    );
    assert!(fixture.show("release/0.1:src/retry.rs").is_some());
    assert_eq!(fixture.show("release/0.1:changelog.d"), None);
}

#[test]
fn a_conflict_stops_the_pick_until_it_is_resolved_and_continued() {
    let fixture = cut_0_1_0();
    fixture.write("src/lib.rs", "two\n");
    fixture.commit("Change the library");
    fixture.write("src/lib.rs", "three\n");
    fixture.fragment("three", "fixed", "Three.");
    let hash = fixture.commit("fix: Change the library again (#10)");
    fixture.push();
    let short = &hash[..7];

    let error = run(&fixture, |pipeline| {
        pick(pipeline, &version("0.1.0"), slice::from_ref(&hash))
    })
    .unwrap_err();

    let worktree = stopped(&fixture);
    assert_eq!(
        error,
        format!(
            "Picking {short} (fix: Change the library again (#10)) stopped at a conflict in \
             src/lib.rs. Resolve it in {worktree}, stage the result with `git add`, and run `jp \
             release pick --continue`."
        )
    );
    // Nothing is pushed while it is stopped, and another pick waits for it.
    assert_eq!(branches(&fixture), [
        "main",
        "release-cut/v0.1.0",
        "release/0.1"
    ]);
    let waiting = run(&fixture, |pipeline| {
        pick(pipeline, &version("0.1.0"), slice::from_ref(&hash))
    })
    .unwrap_err();
    assert!(
        waiting.starts_with("A pick into 0.1.0 stopped at a conflict in "),
        "{waiting}"
    );

    fs::write(worktree.join("src/lib.rs"), "three\n").unwrap();
    fixture
        .shell()
        .git(&worktree, &["add", "src/lib.rs"])
        .unwrap();
    let said = run(&fixture, resume).unwrap();

    assert_eq!(
        said,
        format!(
            "Picked 1 commit onto release/0.1 for release 0.1.0, and pushed it.\nPushed \
             release-pick/v0.1.0-{short}, which files 1 fragment into changelog.d/0.1.0/.\nMerge \
             release-pick/v0.1.0-{short} into main to file them.\n"
        )
    );
    assert_eq!(
        fixture.show("release/0.1:src/lib.rs"),
        Some("three\n".to_owned())
    );
    assert!(
        fixture
            .message("release/0.1")
            .contains(&format!("(cherry picked from commit {hash})"))
    );
    assert_eq!(fixture.show("release/0.1:changelog.d"), None);
    assert!(!worktree.exists());
    assert!(!fixture.root().join(".git").join(STOPPED).exists());
}

#[test]
fn a_conflict_left_unresolved_keeps_the_pick_stopped() {
    let fixture = cut_0_1_0();
    fixture.write("src/lib.rs", "two\n");
    fixture.commit("Change the library");
    fixture.write("src/lib.rs", "three\n");
    let hash = fixture.commit("Change the library again");
    fixture.push();
    run(&fixture, |pipeline| {
        pick(pipeline, &version("0.1.0"), slice::from_ref(&hash))
    })
    .unwrap_err();
    let worktree = stopped(&fixture);

    let error = run(&fixture, resume).unwrap_err();

    assert_eq!(
        error,
        format!(
            "{worktree} still has conflicts in src/lib.rs; resolve them, stage the result with \
             `git add`, and run `jp release pick --continue` again."
        )
    );
    assert!(worktree.exists());
}

/// Removing the worktree drops the stopped pick, and a new pick can run.
#[test]
fn a_stopped_pick_whose_worktree_is_gone_is_dropped() {
    let fixture = cut_0_1_0();
    fixture.write("src/lib.rs", "two\n");
    fixture.commit("Change the library");
    fixture.write("src/lib.rs", "three\n");
    let conflicting = fixture.commit("Change the library again");
    let other = change(&fixture, "src/other.rs", "other", "fix: Another fix (#11)");
    run(&fixture, |pipeline| {
        pick(pipeline, &version("0.1.0"), slice::from_ref(&conflicting))
    })
    .unwrap_err();
    let worktree = stopped(&fixture);
    fixture.git(&["worktree", "remove", "--force", worktree.as_str()]);

    assert_eq!(
        run(&fixture, resume),
        Err(format!(
            "The worktree the pick into 0.1.0 stopped in, {worktree}, is gone; pick the commits \
             again."
        ))
    );
    assert_eq!(
        run(&fixture, resume),
        Err("No pick is stopped, so there is nothing to continue.".to_owned())
    );
    run(&fixture, |pipeline| {
        pick(pipeline, &version("0.1.0"), slice::from_ref(&other))
    })
    .unwrap();
}

/// A pick that conflicts in the code and in the notes stops for the code alone:
/// the notes are left out before the user is asked to resolve anything.
#[test]
fn a_pick_stops_only_for_conflicts_in_the_code() {
    let fixture = cut_0_1_0();
    fixture.write("src/lib.rs", "two\n");
    fixture.commit("Change the library");
    fixture.write("src/lib.rs", "three\n");
    // `retry.md` is on `main`, and the release branch has no notes, so editing
    // it conflicts too.
    fixture.fragment("retry", "fixed", "Retried, three times.");
    let hash = fixture.commit("fix: Retry three times (#14)");
    fixture.push();

    let error = run(&fixture, |pipeline| {
        pick(pipeline, &version("0.1.0"), slice::from_ref(&hash))
    })
    .unwrap_err();

    assert!(
        error.contains("stopped at a conflict in src/lib.rs. Resolve it in "),
        "{error}"
    );
    let worktree = stopped(&fixture);
    fs::write(worktree.join("src/lib.rs"), "three\n").unwrap();
    fixture
        .shell()
        .git(&worktree, &["add", "src/lib.rs"])
        .unwrap();

    let said = run(&fixture, resume).unwrap();

    assert!(
        said.starts_with("Picked 1 commit onto release/0.1 for release 0.1.0, and pushed it.\n"),
        "{said}"
    );
    assert_eq!(
        fixture.show("release/0.1:src/lib.rs"),
        Some("three\n".to_owned())
    );
    assert_eq!(fixture.show("release/0.1:changelog.d"), None);
}

#[test]
fn the_commits_are_picked_in_the_order_main_has_them() {
    let fixture = cut_0_1_0();
    let first = change(&fixture, "src/first.rs", "first", "fix: First (#12)");
    let second = change(&fixture, "src/second.rs", "second", "fix: Second (#13)");

    run(&fixture, |pipeline| {
        pick(pipeline, &version("0.1.0"), &[
            second.clone(),
            first.clone(),
        ])
    })
    .unwrap();

    assert!(
        fixture
            .message("release/0.1")
            .contains(&format!("(cherry picked from commit {second})"))
    );
    assert!(
        fixture
            .message("release/0.1~1")
            .contains(&format!("(cherry picked from commit {first})"))
    );
}
