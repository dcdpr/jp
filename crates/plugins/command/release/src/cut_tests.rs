use serde_json::{Value, json};

use super::*;
use crate::{options::Options, repository_testing::Fixture};

const HIGHLIGHT: &str =
    "# Tool calls fan out\n\n- **Authors**: @JeanMertz\n\nThe assistant asks once.\n";

const BUMP: &str = "git config --file VERSION version.current {version}";

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

#[test]
fn a_cut_makes_the_release_branch_and_its_notes_branch_here_and_pushes_nothing() {
    let fixture = Fixture::new();
    fixture.fragment("retry-idle-turns", "fixed", "Idle turns are retried.");
    fixture.write("changelog.d/highlights/fan-out.md", HIGHLIGHT);
    let main = fixture.commit("fix(llm): Retry idle turns (#1207)");
    fixture.push();
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options).unwrap();

    let said = cut(&pipeline, None, None).unwrap();

    assert_eq!(
        said,
        format!(
            "Started release 0.1.0 on release/0.1, cut from main at {}.\nFiled 1 fragment and 1 \
             highlight into changelog.d/0.1.0/ on release-notes/v0.1.0.\n\nNothing is published \
             yet. Publish both branches with:\n\n    git push origin release/0.1 \
             release-notes/v0.1.0\n\nthen prepare the release's notes on release-notes/v0.1.0, \
             and merge it into main once they are final.\n",
            &main[..7]
        )
    );
    assert_eq!(fixture.origin_branches(), ["main"]);
    assert_eq!(fixture.branches(), [
        "main",
        "release-notes/v0.1.0",
        "release/0.1"
    ]);

    // `main` at the cut, then the commit that starts the release: no
    // fragments, and the version written.
    assert_eq!(fixture.hash("release/0.1^"), main);
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
    let notes = "release-notes/v0.1.0";
    assert_eq!(fixture.hash(&format!("{notes}^")), main);
    assert_eq!(
        fixture.show(&format!(
            "{notes}:changelog.d/0.1.0/retry-idle-turns.fixed.md"
        )),
        Some("Idle turns are retried.\n".to_owned())
    );
    assert_eq!(
        fixture.show(&format!("{notes}:changelog.d/0.1.0/highlights/fan-out.md")),
        Some(HIGHLIGHT.to_owned())
    );
    assert_eq!(
        fixture.show(&format!("{notes}:changelog.d/retry-idle-turns.fixed.md")),
        None
    );
    assert!(
        fixture
            .show(&format!("{notes}:changelog.d/README.md"))
            .is_some()
    );
    assert!(
        fixture
            .show(&format!("{notes}:VERSION"))
            .unwrap()
            .contains("current = 0.1.0")
    );
    assert_eq!(
        fixture.message(notes).trim_end(),
        format!(
            "chore(release): Cut release 0.1.0\n\n`jp release cut` started release 0.1.0 on \
             `release/0.1`, from main at {}.\n\nThis moves the 1 fragment and 1 highlight it \
             ships into `changelog.d/0.1.0/`, where its notes are prepared, and writes the \
             version 0.1.0.",
            &main[..7]
        )
    );

    // The checkout it ran from is left alone, and its worktrees are gone.
    assert_eq!(fixture.git(&["status", "--porcelain"]), "");
    assert_eq!(fixture.git(&["worktree", "list"]).lines().count(), 1);
}

/// JP tagged v0.1.0 before the pipeline existed.
#[test]
fn a_cut_after_a_tag_is_the_next_minor_release() {
    let fixture = Fixture::new();
    fixture.git(&["tag", "v0.1.0"]);
    fixture.publish(&["v0.1.0"]);
    fixture.fragment("retry", "fixed", "Retried.");
    fixture.commit("fix: Retry (#1)");
    fixture.push();
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options).unwrap();

    let said = cut(&pipeline, None, None).unwrap();

    assert!(
        said.starts_with("Started release 0.2.0 on release/0.2,"),
        "{said}"
    );
}

/// A fragment still at the top level because 0.2.0's notes haven't merged is
/// shipped already, and left for 0.2.0's notes branch.
#[test]
fn a_fragment_another_release_ships_waits_for_its_notes_branch() {
    let fixture = Fixture::new();
    fixture.fragment("retry", "fixed", "Retried.");
    fixture.commit("fix: Retry (#1)");
    fixture.push();
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options).unwrap();
    cut(&pipeline, Some(&version("0.2.0")), None).unwrap();
    fixture.publish(&["release/0.2", "release-notes/v0.2.0"]);
    fixture.fragment("model", "added", "Shows the model.");
    fixture.commit("feat: Show the model (#2)");
    fixture.push();

    let said = cut(&pipeline, None, None).unwrap();

    assert!(
        said.starts_with("Started release 0.3.0 on release/0.3,"),
        "{said}"
    );
    assert!(
        said.contains("Filed 1 fragment into changelog.d/0.3.0/ on release-notes/v0.3.0."),
        "{said}"
    );
    assert!(
        fixture
            .show("release-notes/v0.3.0:changelog.d/0.3.0/model.added.md")
            .is_some()
    );
    assert!(
        fixture
            .show("release-notes/v0.3.0:changelog.d/retry.fixed.md")
            .is_some()
    );
}

#[test]
fn from_1_0_on_a_breaking_change_cuts_a_major_release() {
    let fixture = Fixture::new();
    fixture.git(&["tag", "v1.2.0"]);
    fixture.publish(&["v1.2.0"]);
    fixture.write(
        "changelog.d/manifest.changed.md",
        "Plugins are found by their manifest.\n\nBREAKING CHANGE: Plugins need a \
         manifest\n\nRebuild them.\n",
    );
    fixture.commit("feat!: Route plugins by manifest (#3)");
    fixture.push();
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options).unwrap();

    let said = cut(&pipeline, None, None).unwrap();

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
    fixture.publish(&["v1.2.0"]);
    fixture.write(
        "changelog.d/manifest.changed.md",
        "Plugins are found by their manifest.\n\nBREAKING CHANGE: Plugins need a \
         manifest\n\nRebuild them.\n",
    );
    fixture.fragment("retry", "fixed", "Retried.");
    fixture.commit("feat!: Route plugins by manifest (#3)");
    fixture.push();
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options).unwrap();

    assert_eq!(
        cut(&pipeline, Some(&version("1.3.0")), None),
        Err(
            "1.3.0 is a minor release from 1.0 on, which can't ship a change that requires users \
             to act, and changelog.d/manifest.changed.md has an upgrade section. Leave out \
             `--version` to cut the next major release."
                .to_owned()
        )
    );
    assert_eq!(fixture.branches(), ["main"]);

    // A major release may.
    let said = cut(&pipeline, Some(&version("2.0.0")), None).unwrap();
    assert!(
        said.starts_with("Started release 2.0.0 on release/2.0,"),
        "{said}"
    );
}

#[test]
fn a_main_without_unreleased_fragments_has_nothing_to_release() {
    let fixture = Fixture::new();
    fixture.write("changelog.d/highlights/fan-out.md", HIGHLIGHT);
    let main = fixture.commit("Describe a feature");
    fixture.push();
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options).unwrap();

    assert_eq!(
        cut(&pipeline, None, None),
        Err(format!(
            "main at {} has no unreleased fragments, so there is nothing to release.",
            &main[..7]
        ))
    );
    assert_eq!(fixture.branches(), ["main"]);
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
    let pipeline = Pipeline::new(&shell, fixture.root(), &options).unwrap();

    assert_eq!(
        cut(&pipeline, None, None),
        Err("The fragments on `main` break their format; fix these \
             first:\nchangelog.d/notes.txt: not a fragment, which is named `<slug>.<type>.md`; \
             rename it, or move it out of the fragments directory"
            .to_owned())
    );
    assert_eq!(fixture.branches(), ["main"]);
}

/// A rehearsal's leftovers, or a cut nobody published, would count as releases
/// made, so the next cut waits for them to be published or deleted.
#[test]
fn a_cut_refuses_while_a_release_exists_only_here() {
    let fixture = Fixture::new();
    fixture.fragment("retry", "fixed", "Retried.");
    fixture.commit("fix: Retry (#1)");
    fixture.push();
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options).unwrap();
    cut(&pipeline, None, None).unwrap();
    fixture.git(&["tag", "v0.0.9", "main"]);

    assert_eq!(
        cut(&pipeline, None, None),
        Err(
            "Only this repository has release/0.1 and v0.0.9, so a cut would take them for \
             releases already made. Publish them with `git push origin release/0.1 v0.0.9`, or \
             delete them with `git branch -D release/0.1` and `git tag -d v0.0.9`, and cut again."
                .to_owned()
        )
    );

    // Published, they are releases like any other.
    fixture.git(&["tag", "--delete", "v0.0.9"]);
    fixture.publish(&["release/0.1", "release-notes/v0.1.0"]);
    fixture.fragment("model", "added", "Shows the model.");
    fixture.commit("feat: Show the model (#2)");
    fixture.push();

    let said = cut(&pipeline, None, None).unwrap();
    assert!(
        said.starts_with("Started release 0.2.0 on release/0.2,"),
        "{said}"
    );
}

#[test]
fn a_cut_refuses_a_notes_branch_an_earlier_cut_left() {
    let fixture = Fixture::new();
    fixture.fragment("retry", "fixed", "Retried.");
    fixture.commit("fix: Retry (#1)");
    fixture.push();
    fixture.git(&["branch", "release-notes/v0.1.0"]);
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options).unwrap();

    assert_eq!(
        cut(&pipeline, None, None),
        Err(
            "release-notes/v0.1.0 exists already; delete it if an earlier cut left it behind, \
             here with `git branch -D release-notes/v0.1.0` or on origin with `git push origin \
             --delete release-notes/v0.1.0`."
                .to_owned()
        )
    );
    assert_eq!(fixture.branches(), ["main", "release-notes/v0.1.0"]);
}

/// A rehearsal on a branch of its own cuts from that branch, and says the cut
/// holds commits `main` doesn't.
#[test]
fn a_cut_from_a_checkout_ahead_of_main_says_so() {
    let fixture = Fixture::new();
    let main = fixture.hash("main");
    fixture.git(&["switch", "--quiet", "--create", "rehearsal"]);
    fixture.fragment("retry", "fixed", "Retried.");
    let rehearsal = fixture.commit("fix: Retry (#1)");
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options).unwrap();

    let said = cut(&pipeline, None, None).unwrap();

    assert!(
        said.starts_with(&format!(
            "Started release 0.1.0 on release/0.1, cut from rehearsal at {}.\nrehearsal at {} is \
             ahead of origin's main at {}: release/0.1 and release-notes/v0.1.0 hold commits main \
             doesn't have yet.\n",
            &rehearsal[..7],
            &rehearsal[..7],
            &main[..7]
        )),
        "{said}"
    );
    assert_eq!(fixture.hash("release/0.1^"), rehearsal);
    assert_eq!(fixture.origin_branches(), ["main"]);
}

/// A checkout without `main`'s latest commits would cut a release without them.
#[test]
fn a_cut_refuses_a_checkout_behind_main() {
    let fixture = Fixture::new();
    let behind = fixture.hash("main");
    fixture.fragment("retry", "fixed", "Retried.");
    let main = fixture.commit("fix: Retry (#1)");
    fixture.push();
    fixture.git(&["reset", "--quiet", "--hard", &behind]);
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options).unwrap();

    assert_eq!(
        cut(&pipeline, None, None),
        Err(format!(
            "main at {} lacks commits origin's main has, at {}; bring them in first, such as with \
             `git pull`.",
            &behind[..7],
            &main[..7]
        ))
    );
    assert_eq!(fixture.branches(), ["main"]);
}

/// A workspace below the top of its repository keeps its fragments, and runs
/// `bump`, there.
#[test]
fn a_workspace_below_the_top_of_its_repository_is_released_from_there() {
    let fixture = Fixture::new();
    fixture.write("app/changelog.d/retry.fixed.md", "Retried.\n");
    fixture.commit("fix: Retry (#1)");
    fixture.push();
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, &fixture.root().join("app"), &options).unwrap();

    cut(&pipeline, None, None).unwrap();

    assert!(
        fixture
            .show("release-notes/v0.1.0:app/changelog.d/0.1.0/retry.fixed.md")
            .is_some()
    );
    assert!(fixture.show("release-notes/v0.1.0:app/VERSION").is_some());
    assert_eq!(fixture.show("release/0.1:app/changelog.d"), None);
    // The fragments directory of another workspace in the repository stays.
    assert!(fixture.show("release/0.1:changelog.d/README.md").is_some());
}

/// History from before the project kept fragments is released one period at a
/// time: fragments written later name the changes they describe in their
/// `Credits`, and a cut at an earlier commit files only the ones its history
/// holds.
#[test]
fn a_cut_at_an_earlier_commit_files_the_fragments_its_history_holds() {
    let fixture = Fixture::new();
    fixture.git(&["rm", "-r", "--quiet", "changelog.d"]);
    fixture.write("src/model.rs", "// Shows the model.\n");
    let first = fixture.commit("feat: Show the model");
    fixture.write("src/retry.rs", "// Retries.\n");
    let second = fixture.commit("fix: Retry idle turns");
    fixture.write("changelog.d/README.md", "# Changelog fragments\n");
    fixture.write(
        "changelog.d/model.added.md",
        &format!("- **Credits**: {}\n\nShows the model.\n", &first[..7]),
    );
    fixture.write(
        "changelog.d/retry.fixed.md",
        &format!("- **Credits**: {}\n\nRetries idle turns.\n", &second[..7]),
    );
    let main = fixture.commit("docs: Describe the first releases");
    fixture.push();
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options).unwrap();

    let said = cut(&pipeline, None, Some(&first[..7])).unwrap();

    assert_eq!(
        said,
        format!(
            "Started release 0.1.0 on release/0.1, cut from {}, an earlier commit of main at \
             {}.\nFiled 1 fragment into changelog.d/0.1.0/ on release-notes/v0.1.0.\n\nNothing is \
             published yet. Publish both branches with:\n\n    git push origin release/0.1 \
             release-notes/v0.1.0\n\nthen prepare the release's notes on release-notes/v0.1.0, \
             and merge it into main once they are final.\n",
            &first[..7],
            &main[..7]
        )
    );
    // The code as it was then, when there was no fragments directory to
    // remove.
    assert_eq!(fixture.hash("release/0.1^"), first);
    assert_eq!(fixture.show("release/0.1:changelog.d"), None);
    assert_eq!(fixture.show("release/0.1:src/retry.rs"), None);
    assert!(
        fixture
            .show("release/0.1:VERSION")
            .unwrap()
            .contains("current = 0.1.0")
    );
    // The notes, from the checkout, filing only the fragment of a change the
    // release ships.
    let notes = "release-notes/v0.1.0";
    assert_eq!(fixture.hash(&format!("{notes}^")), main);
    assert!(
        fixture
            .show(&format!("{notes}:changelog.d/0.1.0/model.added.md"))
            .is_some()
    );
    assert!(
        fixture
            .show(&format!("{notes}:changelog.d/retry.fixed.md"))
            .is_some()
    );

    // The next period's release files the rest.
    fixture.publish(&["release/0.1", "release-notes/v0.1.0"]);
    fixture.merge_notes("0.1.0", "chore(release): Cut release 0.1.0");

    let said = cut(&pipeline, None, Some(&second)).unwrap();

    assert!(
        said.starts_with("Started release 0.2.0 on release/0.2,"),
        "{said}"
    );
    assert_eq!(fixture.hash("release/0.2^"), second);
    assert!(
        fixture
            .show("release-notes/v0.2.0:changelog.d/0.2.0/retry.fixed.md")
            .is_some()
    );
}

/// On a forge, a change request in `Credits` stands for the commit that merged
/// it.
#[test]
fn a_change_request_credit_names_the_commit_that_merged_it() {
    let fixture = Fixture::new();
    fixture.write("src/model.rs", "// Shows the model.\n");
    let merged = fixture.commit("feat: Show the model (#12)");
    fixture.write("src/later.rs", "// Later.\n");
    fixture.commit("feat: Do something later (#13)");
    fixture.write(
        "changelog.d/model.added.md",
        "- **Credits**: #12\n\nShows the model.\n",
    );
    fixture.write(
        "changelog.d/later.added.md",
        "- **Credits**: #13\n\nDoes something later.\n",
    );
    fixture.commit("docs: Describe the first release");
    fixture.push();
    let shell = fixture.shell();
    let options = options_with(&json!({ "forge": "github", "bump": BUMP }));
    let pipeline = Pipeline::new(&shell, fixture.root(), &options).unwrap();

    let said = cut(&pipeline, None, Some(&merged)).unwrap();

    assert!(
        said.contains("Filed 1 fragment into changelog.d/0.1.0/"),
        "{said}"
    );
    assert!(
        fixture
            .show("release-notes/v0.1.0:changelog.d/0.1.0/model.added.md")
            .is_some()
    );
    assert!(
        fixture
            .show("release-notes/v0.1.0:changelog.d/later.added.md")
            .is_some()
    );
}

/// A credit nothing answers to would file its fragment by guesswork, so the cut
/// stops at it.
#[test]
fn a_credit_no_commit_answers_to_stops_the_cut() {
    let fixture = Fixture::new();
    fixture.write(
        "changelog.d/model.added.md",
        "- **Credits**: #99\n\nShows the model.\n",
    );
    fixture.write(
        "changelog.d/retry.fixed.md",
        "- **Credits**: abcdef0\n\nRetried.\n",
    );
    fixture.commit("docs: Describe two changes");
    fixture.push();
    let shell = fixture.shell();
    let github = options_with(&json!({ "forge": "github", "bump": BUMP }));
    let pipeline = Pipeline::new(&shell, fixture.root(), &github).unwrap();

    assert_eq!(
        cut(&pipeline, None, None),
        Err(
            "changelog.d/model.added.md credits #99, which no commit in main's history merged; \
             correct its `Credits`."
                .to_owned()
        )
    );

    // Without a forge, a change request finds no commit, and only the commit
    // credited counts.
    let none = options();
    let pipeline = Pipeline::new(&shell, fixture.root(), &none).unwrap();
    assert_eq!(
        cut(&pipeline, None, None),
        Err(
            "changelog.d/retry.fixed.md credits abcdef0, which names no commit this repository \
             has; correct its `Credits`."
                .to_owned()
        )
    );
    assert_eq!(fixture.branches(), ["main"]);
}

#[test]
fn a_cut_at_a_commit_no_fragment_describes_files_nothing() {
    let fixture = Fixture::new();
    let start = fixture.hash("main");
    fixture.fragment("retry", "fixed", "Retried.");
    let main = fixture.commit("fix: Retry (#1)");
    fixture.push();
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options).unwrap();

    assert_eq!(
        cut(&pipeline, None, Some(&start)),
        Err(format!(
            "None of the unreleased fragments on main at {} describes a change {} has, so a \
             release cut there would list nothing; write fragments whose `Credits` name its \
             changes first.",
            &main[..7],
            &start[..7]
        ))
    );
    assert_eq!(fixture.branches(), ["main"]);
}

/// A release is cut at a commit of `main`'s own history that no release ships
/// yet.
#[test]
fn a_cut_at_refuses_a_commit_off_main_or_shipped_already() {
    let fixture = Fixture::new();
    let start = fixture.hash("main");
    fixture.git(&["switch", "--quiet", "--create", "side"]);
    fixture.write("src/side.rs", "// A side change.\n");
    let side = fixture.commit("Change something on the side");
    fixture.git(&["switch", "--quiet", "main"]);
    fixture.git(&["merge", "--quiet", "--no-ff", "--no-edit", "side"]);
    fixture.fragment("retry", "fixed", "Retried.");
    let main = fixture.commit("fix: Retry (#1)");
    fixture.push();
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options).unwrap();

    assert_eq!(
        cut(&pipeline, None, Some(&side)),
        Err(format!(
            "{} isn't on the first-parent line of main at {}, so it isn't a commit of main's own \
             history; a release is cut at one of those.",
            &side[..7],
            &main[..7]
        ))
    );
    assert_eq!(
        cut(&pipeline, None, Some("no-such-commit")),
        Err("`no-such-commit` names no commit.".to_owned())
    );

    cut(&pipeline, None, None).unwrap();
    fixture.publish(&["release/0.1", "release-notes/v0.1.0"]);

    assert_eq!(
        cut(&pipeline, None, Some(&start)),
        Err(format!(
            "release/0.1 ships {} already, so a release cut there would ship nothing new; name a \
             later commit.",
            &start[..7]
        ))
    );
}

/// The error names the setting `bump` comes from, before anything is made.
#[test]
fn a_bump_that_fails_names_its_setting() {
    let fixture = Fixture::new();
    fixture.fragment("retry", "fixed", "Retried.");
    fixture.commit("fix: Retry (#1)");
    fixture.push();
    let shell = fixture.shell();
    let options = options_with(&json!({ "forge": "none", "bump": "git set-version {version}" }));
    let pipeline = Pipeline::new(&shell, fixture.root(), &options).unwrap();

    let error = cut(&pipeline, None, None).unwrap_err();

    assert!(
        error.starts_with(
            "`plugins.command.release.options.bump` failed writing 0.1.0: git: 'set-version' is \
             not a git command."
        ),
        "{error}"
    );
    assert_eq!(fixture.branches(), ["main"]);
}
