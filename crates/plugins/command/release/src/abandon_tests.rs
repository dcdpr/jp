use std::slice;

use serde_json::{Value, json};

use super::*;
use crate::{
    cut::cut, finalize::finalize, options::Options, pick::pick, repository_testing::Fixture,
};

/// A `bump` that writes the version into `VERSION`, wherever git runs.
const BUMP: &str = "git config --file VERSION version.current {version}";

/// The options of a project on `forge`, with [`BUMP`].
fn options_for(forge: &str) -> Options {
    let Value::Object(options) = json!({ "forge": forge, "bump": BUMP }) else {
        panic!("options are an object");
    };

    Options::read(&options).unwrap()
}

fn version(text: &str) -> Version {
    Version::parse(text).unwrap()
}

/// Run `command` in the workspace of `fixture`, without a forge.
fn run(
    fixture: &Fixture,
    command: impl FnOnce(&Pipeline<'_>) -> Result<String, String>,
) -> Result<String, String> {
    run_with(fixture, &options_for("none"), command)
}

/// Run `command` in the workspace of `fixture` with `options`.
fn run_with(
    fixture: &Fixture,
    options: &Options,
    command: impl FnOnce(&Pipeline<'_>) -> Result<String, String>,
) -> Result<String, String> {
    let shell = fixture.shell();
    let pipeline = Pipeline::new(&shell, fixture.root(), options).unwrap();

    command(&pipeline)
}

/// A workspace whose release 0.1.0, listing one fix, was cut here, nothing of
/// it published.
fn cut_0_1_0() -> Fixture {
    let fixture = Fixture::new();
    fixture.fragment("retry", "fixed", "Retried.");
    fixture.commit("fix: Retry (#1)");
    fixture.push();
    run(&fixture, |pipeline| cut(pipeline, None, None)).unwrap();

    fixture
}

/// [`cut_0_1_0`], with both branches published and the notes merged into
/// `main`.
fn filed_0_1_0() -> Fixture {
    let fixture = cut_0_1_0();
    fixture.publish(&["release/0.1", "release-notes/v0.1.0"]);
    fixture.merge_notes("0.1.0", "chore(release): Cut release 0.1.0");

    fixture
}

/// The notes branch hasn't merged, so the notes come back from it, as they were
/// edited there.
#[test]
fn a_cut_nobody_published_is_taken_back_with_its_notes_as_edited() {
    let fixture = cut_0_1_0();
    fixture.git(&["switch", "--quiet", "release-notes/v0.1.0"]);
    fixture.write(
        "changelog.d/0.1.0/retry.fixed.md",
        "Idle turns are retried.\n",
    );
    fixture.commit("Reword the fix");
    fixture.git(&["switch", "--quiet", "main"]);

    let said = run(&fixture, |pipeline| abandon(pipeline, &version("0.1.0"))).unwrap();

    assert_eq!(
        said,
        "Abandoned 0.1.0: deleted release/0.1 and release-notes/v0.1.0 here.\nOn \
         release-notes/abandon-v0.1.0, moved 1 fragment back to the top of \
         changelog.d/.\n\nNothing is pushed yet. Run:\n\n    git push origin \
         release-notes/abandon-v0.1.0\n\nthen merge release-notes/abandon-v0.1.0 into main, so \
         0.1.0 can be cut again.\n"
    );
    assert_eq!(fixture.branches(), ["main", "release-notes/abandon-v0.1.0"]);

    let branch = "release-notes/abandon-v0.1.0";
    assert_eq!(
        fixture.show(&format!("{branch}:changelog.d/retry.fixed.md")),
        Some("Idle turns are retried.\n".to_owned())
    );
    assert_eq!(
        fixture.show(&format!("{branch}:changelog.d/0.1.0/retry.fixed.md")),
        None
    );
    assert_eq!(
        fixture.message(branch).trim_end(),
        "chore(release): Abandon release 0.1.0\n\n`jp release abandon` took back release 0.1.0, \
         so it can be cut again.\n\nThis moves the 1 fragment its notes filed back to the top of \
         `changelog.d/`."
    );
    assert_eq!(fixture.git(&["worktree", "list"]).lines().count(), 1);
}

/// `main` has the fragments of a cut whose notes never merged, as they were, so
/// there is nothing to bring back, and the release cuts again at once.
#[test]
fn a_cut_whose_notes_nobody_edited_leaves_nothing_to_merge() {
    let fixture = cut_0_1_0();

    let said = run(&fixture, |pipeline| abandon(pipeline, &version("0.1.0"))).unwrap();

    assert_eq!(
        said,
        "Abandoned 0.1.0: deleted release/0.1 and release-notes/v0.1.0 here.\nNone of the notes \
         of 0.1.0 has to come back to the top of changelog.d/.\n\n0.1.0 can be cut again.\n"
    );
    assert_eq!(fixture.branches(), ["main"]);

    let said = run(&fixture, |pipeline| cut(pipeline, None, None)).unwrap();
    assert!(
        said.starts_with("Started release 0.1.0 on release/0.1,"),
        "{said}"
    );
}

/// Once its notes merged, they come back from `main`, and the release cuts
/// again once `origin` and `main` have caught up.
#[test]
fn a_published_cut_whose_notes_merged_moves_them_back_from_main() {
    let fixture = filed_0_1_0();
    fixture.write("changelog.d/0.1.0/overview.md", "JP 0.1.0 retries.\n");
    let main = fixture.commit("Write the overview of 0.1.0");
    fixture.push();

    let said = run(&fixture, |pipeline| abandon(pipeline, &version("0.1.0"))).unwrap();

    assert_eq!(
        said,
        "Abandoned 0.1.0: deleted release/0.1 and release-notes/v0.1.0 here.\nOn \
         release-notes/abandon-v0.1.0, moved 1 fragment back to the top of \
         changelog.d/.\n\nNothing is pushed yet. Run:\n\n    git push origin --delete release/0.1 \
         release-notes/v0.1.0\n    git push origin release-notes/abandon-v0.1.0\n\nthen merge \
         release-notes/abandon-v0.1.0 into main, so 0.1.0 can be cut again.\n"
    );
    let branch = "release-notes/abandon-v0.1.0";
    assert_eq!(fixture.hash(&format!("{branch}^")), main);
    assert!(
        fixture
            .show(&format!("{branch}:changelog.d/retry.fixed.md"))
            .is_some()
    );
    // The overview waits for the next cut of 0.1.0.
    assert_eq!(
        fixture.show(&format!("{branch}:changelog.d/0.1.0/overview.md")),
        Some("JP 0.1.0 retries.\n".to_owned())
    );

    fixture.publish(&["--delete", "release/0.1", "release-notes/v0.1.0"]);
    fixture.git(&["merge", "--quiet", "--squash", branch]);
    fixture.commit("chore(release): Abandon release 0.1.0");
    fixture.push();

    let said = run(&fixture, |pipeline| cut(pipeline, None, None)).unwrap();
    assert!(
        said.starts_with("Started release 0.1.0 on release/0.1,"),
        "{said}"
    );
    assert!(
        fixture
            .show("release-notes/v0.1.0:changelog.d/0.1.0/retry.fixed.md")
            .is_some()
    );
}

#[test]
fn a_published_release_is_not_abandoned() {
    let fixture = filed_0_1_0();
    run(&fixture, |pipeline| finalize(pipeline, &version("0.1.0"))).unwrap();
    fixture.publish(&["v0.1.0"]);

    assert_eq!(
        run(&fixture, |pipeline| abandon(pipeline, &version("0.1.0"))),
        Err(
            "origin has the tag v0.1.0, so 0.1.0 is published, and a published release can't be \
             taken back."
                .to_owned()
        )
    );
}

/// A tag nobody pushed goes with the rest of the release.
#[test]
fn a_tag_nobody_published_is_deleted_too() {
    let fixture = filed_0_1_0();
    run(&fixture, |pipeline| finalize(pipeline, &version("0.1.0"))).unwrap();

    let said = run(&fixture, |pipeline| abandon(pipeline, &version("0.1.0"))).unwrap();

    assert!(
        said.starts_with(
            "Abandoned 0.1.0: deleted release/0.1, release-notes/v0.1.0, and the tag v0.1.0 \
             here.\n"
        ),
        "{said}"
    );
    assert_eq!(fixture.git(&["tag", "--list"]), "");
}

#[test]
fn releases_are_abandoned_newest_first() {
    let fixture = cut_0_1_0();
    fixture.publish(&["release/0.1", "release-notes/v0.1.0"]);
    fixture.fragment("model", "added", "Shows the model.");
    fixture.commit("feat: Show the model (#2)");
    fixture.push();
    run(&fixture, |pipeline| cut(pipeline, None, None)).unwrap();

    assert_eq!(
        run(&fixture, |pipeline| abandon(pipeline, &version("0.1.0"))),
        Err(
            "0.2.0 is newer, and releases are abandoned newest first: abandon 0.2.0, and run the \
             pushes that prints, before 0.1.0."
                .to_owned()
        )
    );
    assert_eq!(
        run(&fixture, |pipeline| abandon(pipeline, &version("0.3.0"))),
        Err("0.3.0 hasn't been cut, so there is nothing to abandon.".to_owned())
    );
}

/// A patch release's commits come off its branch, which is back where the
/// release before it left it.
#[test]
fn abandoning_a_patch_release_takes_its_commits_off_its_branch() {
    let fixture = filed_0_1_0();
    run(&fixture, |pipeline| finalize(pipeline, &version("0.1.0"))).unwrap();
    fixture.publish(&["v0.1.0"]);
    let tagged = fixture.hash("v0.1.0^{commit}");
    fixture.write("src/fix.rs", "// A fix.\n");
    fixture.fragment("fix", "fixed", "A fix.");
    let hash = fixture.commit("fix: A fix (#3)");
    fixture.push();
    run(&fixture, |pipeline| {
        pick(pipeline, &version("0.1.1"), slice::from_ref(&hash))
    })
    .unwrap();

    let said = run(&fixture, |pipeline| abandon(pipeline, &version("0.1.1"))).unwrap();

    assert_eq!(
        said,
        format!(
            "Abandoned 0.1.1: took its commits off release/0.1, which is back at {}, and deleted \
             release-notes/v0.1.1 here.\nNone of the notes of 0.1.1 has to come back to the top \
             of changelog.d/.\n\n0.1.1 can be picked into again.\n",
            &tagged[..7]
        )
    );
    assert_eq!(fixture.hash("release/0.1"), tagged);
    assert_eq!(fixture.branches(), [
        "main",
        "release-notes/v0.1.0",
        "release/0.1"
    ]);
}

/// A file `pick` copied into the release, because a later release ships its
/// change too, goes, and the original stays where it is.
#[test]
fn a_copy_pick_made_is_removed() {
    let fixture = filed_0_1_0();
    run(&fixture, |pipeline| finalize(pipeline, &version("0.1.0"))).unwrap();
    fixture.publish(&["v0.1.0"]);
    fixture.write("src/fix.rs", "// A fix.\n");
    fixture.fragment("fix", "fixed", "A fix.");
    let hash = fixture.commit("fix: A fix (#3)");
    fixture.push();
    let github = options_for("github");
    // 0.2.0 ships the fix, so picking it into 0.1.1 copies its fragment.
    run_with(&fixture, &github, |pipeline| cut(pipeline, None, None)).unwrap();
    run_with(&fixture, &github, |pipeline| {
        pick(pipeline, &version("0.1.1"), slice::from_ref(&hash))
    })
    .unwrap();
    fixture.merge_notes("0.1.1", "chore(release): Pick a fix into release 0.1.1");
    run(&fixture, |pipeline| abandon(pipeline, &version("0.2.0"))).unwrap();

    let said = run(&fixture, |pipeline| abandon(pipeline, &version("0.1.1"))).unwrap();

    assert!(
        said.contains(
            "On release-notes/abandon-v0.1.1, removed 1 file `jp release pick` copied from other \
             releases.\n"
        ),
        "{said}"
    );
    let branch = "release-notes/abandon-v0.1.1";
    assert_eq!(
        fixture.show(&format!("{branch}:changelog.d/0.1.1/fix.fixed.md")),
        None
    );
    assert_eq!(
        fixture.show(&format!("{branch}:changelog.d/fix.fixed.md")),
        Some("A fix.\n".to_owned())
    );
}

/// An abandon branch an earlier abandon left would take the notes of a later
/// one, so the abandon waits for it to go.
#[test]
fn an_abandon_branch_left_behind_stops_the_abandon() {
    let fixture = cut_0_1_0();
    fixture.git(&["branch", "release-notes/abandon-v0.1.0"]);

    assert_eq!(
        run(&fixture, |pipeline| abandon(pipeline, &version("0.1.0"))),
        Err(
            "release-notes/abandon-v0.1.0 exists already; delete it if an earlier abandon left it \
             behind, here with `git branch -D release-notes/abandon-v0.1.0` or on origin with \
             `git push origin --delete release-notes/abandon-v0.1.0`."
                .to_owned()
        )
    );
    assert_eq!(fixture.branches(), [
        "main",
        "release-notes/abandon-v0.1.0",
        "release-notes/v0.1.0",
        "release/0.1"
    ]);
}

/// A file at the top level named like one of the release's is a different
/// fragment, which moving the release's back would overwrite.
#[test]
fn a_file_at_the_top_named_alike_stops_the_abandon() {
    let fixture = filed_0_1_0();
    fixture.fragment("retry", "fixed", "Retried twice.");
    fixture.commit("fix: Retry again (#4)");
    fixture.push();

    assert_eq!(
        run(&fixture, |pipeline| abandon(pipeline, &version("0.1.0"))),
        Err(
            "changelog.d/retry.fixed.md exists already, so the file of 0.1.0 with its name can't \
             move back there; rename one of the two, and abandon again."
                .to_owned()
        )
    );
    assert_eq!(fixture.branches(), [
        "main",
        "release-notes/v0.1.0",
        "release/0.1"
    ]);
}
