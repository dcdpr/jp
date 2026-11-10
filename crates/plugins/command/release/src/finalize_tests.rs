use serde_json::{Value, json};

use super::*;
use crate::{cut::cut, options::Options, pick::pick, repository_testing::Fixture};

fn options() -> Options {
    let Value::Object(options) = json!({ "forge": "none" }) else {
        panic!("options are an object");
    };

    Options::read(&options).unwrap()
}

fn version(text: &str) -> Version {
    Version::parse(text).unwrap()
}

/// Finalize release `at` in `fixture`'s workspace.
fn finalized(fixture: &Fixture, at: &str) -> Result<String, String> {
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options).unwrap();

    finalize(&pipeline, &version(at))
}

/// Cut the next release in `fixture`'s workspace.
fn cut_here(fixture: &Fixture) {
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options).unwrap();
    cut(&pipeline, None, None).unwrap();
}

/// The message of the tag `tag` in the workspace's repository.
fn tag_message(fixture: &Fixture, tag: &str) -> String {
    fixture
        .git(&["tag", "--list", "--format=%(contents)", tag])
        .trim_end()
        .to_owned()
}

/// A workspace whose release 0.1.0, listing `Retried.`, was cut here, its
/// branches not yet published.
fn cut_0_1_0() -> Fixture {
    let fixture = Fixture::new();
    fixture.write(
        "changelog.d/retry.fixed.md",
        "- **Credits**: #1\n\nRetried.\n",
    );
    fixture.commit("fix: Retry");
    fixture.push();
    cut_here(&fixture);

    fixture
}

/// [`cut_0_1_0`], with its notes branch merged into `main`, and `main` pushed.
fn filed_0_1_0() -> Fixture {
    let fixture = cut_0_1_0();
    fixture.merge_notes("0.1.0", "chore(release): Cut release 0.1.0 (#2)");

    fixture
}

#[test]
fn finalize_tags_the_tip_of_the_release_branch_with_its_notes_and_pushes_nothing() {
    let fixture = filed_0_1_0();
    fixture.publish(&["release/0.1"]);
    let tip = fixture.hash("release/0.1");
    let head = fixture.hash("HEAD");

    let said = finalized(&fixture, "0.1.0").unwrap();

    assert_eq!(
        said,
        format!(
            "Tagged the tip of release/0.1, {}, as v0.1.0, with the notes of 0.1.0 on main at \
             {}.\n\nNothing is published yet. Push the tag to release it:\n\n    git push origin \
             v0.1.0\n",
            &tip[..7],
            &head[..7]
        )
    );
    assert_eq!(fixture.git(&["cat-file", "-t", "v0.1.0"]).trim(), "tag");
    assert_eq!(fixture.hash("v0.1.0^{commit}"), tip);
    assert_eq!(
        tag_message(&fixture, "v0.1.0"),
        concat!(
            "Release 0.1.0\n",
            "\n",
            "Version 0.1.0 has 1 entry: Fixed (1).\n",
            "\n",
            "## Changelog\n",
            "\n",
            "### Fixed\n",
            "\n",
            "- Retried.\n",
            "  (#1)",
        )
    );
    assert_eq!(
        fixture
            .git(&["tag", "--list", "--format=%(contents:subject)", "v0.1.0"])
            .trim_end(),
        "Release 0.1.0"
    );
    assert!(fixture.origin_tags().is_empty());
}

/// The fixture's `origin` is a path on disk, so the tag's notes link their
/// credits from the configured address, as `render` does.
#[test]
fn the_notes_in_the_tag_link_from_the_configured_repository() {
    let fixture = Fixture::new();
    fixture.fragment("retry", "fixed", "Retried.");
    fixture.commit("fix: Retry (#1)");
    fixture.push();
    let Value::Object(options) = json!({
        "forge": "github",
        "repository": "https://github.com/dcdpr/jp",
    }) else {
        panic!("options are an object");
    };
    let options = Options::read(&options).unwrap();
    let shell = fixture.shell();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options).unwrap();
    cut(&pipeline, None, None).unwrap();
    fixture.merge_notes("0.1.0", "chore(release): Cut release 0.1.0 (#2)");

    finalize(&pipeline, &version("0.1.0")).unwrap();

    assert_eq!(
        tag_message(&fixture, "v0.1.0"),
        concat!(
            "Release 0.1.0\n",
            "\n",
            "Version 0.1.0 has 1 entry: Fixed (1).\n",
            "\n",
            "## Changelog\n",
            "\n",
            "### Fixed\n",
            "\n",
            "- Retried.\n",
            "  ([#1](https://github.com/dcdpr/jp/pull/1))",
        )
    );
}

/// The upgrade guide follows the notes in the same message, so a breaking entry
/// links to its section by anchor; git keeps the `#` headings.
#[test]
fn the_upgrade_guide_follows_the_notes_in_the_tag() {
    let fixture = Fixture::new();
    fixture.write(
        "changelog.d/retry.fixed.md",
        "- **Credits**: #1\n\nRetried.\n",
    );
    fixture.write(
        "changelog.d/manifest.changed.md",
        concat!(
            "- **Credits**: #2\n",
            "\n",
            "Plugins are found by their manifest.\n",
            "\n",
            "BREAKING CHANGE: Plugins are found by their manifest\n",
            "\n",
            "Rebuild your plugins.\n",
        ),
    );
    fixture.commit("feat: Find plugins by their manifest");
    fixture.push();
    cut_here(&fixture);
    fixture.merge_notes("0.1.0", "chore(release): Cut release 0.1.0 (#3)");

    finalized(&fixture, "0.1.0").unwrap();

    assert_eq!(
        tag_message(&fixture, "v0.1.0"),
        concat!(
            "Release 0.1.0\n",
            "\n",
            "Version 0.1.0 has 2 entries: Changed (1) and Fixed (1).\n",
            "\n",
            "## Changelog\n",
            "\n",
            "### Changed\n",
            "\n",
            "- Plugins are found by their manifest.\n",
            "  **Breaking**: see [the upgrade guide](#plugins-are-found-by-their-manifest).\n",
            "  (#2)\n",
            "\n",
            "### Fixed\n",
            "\n",
            "- Retried.\n",
            "  (#1)\n",
            "\n",
            "# Upgrade guide\n",
            "\n",
            "## Plugins are found by their manifest\n",
            "\n",
            "#2\n",
            "\n",
            "Rebuild your plugins.",
        )
    );
}

/// CI runs on what is pushed to a release branch, so a tag on commits `origin`
/// lacks is one CI hasn't checked.
#[test]
fn an_unpushed_release_branch_is_pushed_with_the_tag() {
    let fixture = filed_0_1_0();

    let said = finalized(&fixture, "0.1.0").unwrap();

    assert!(
        said.ends_with(
            "origin lacks commits of release/0.1, so its CI hasn't checked what v0.1.0 points \
             at.\n\nNothing is published yet. Push the tag to release it:\n\n    git push origin \
             release/0.1 v0.1.0\n"
        ),
        "{said}"
    );
}

/// A tag made here and not yet pushed is the release; finalizing again says how
/// to publish it.
#[test]
fn a_release_tagged_here_says_how_to_publish_the_tag() {
    let fixture = filed_0_1_0();
    fixture.publish(&["release/0.1"]);
    finalized(&fixture, "0.1.0").unwrap();

    assert_eq!(
        finalized(&fixture, "0.1.0"),
        Ok(
            "v0.1.0 is tagged at the tip of release/0.1 already, but origin doesn't have \
             it.\n\nNothing is published yet. Push the tag to release it:\n\n    git push origin \
             v0.1.0\n"
                .to_owned()
        )
    );
}

#[test]
fn a_finalized_release_is_not_finalized_twice() {
    let fixture = filed_0_1_0();
    finalized(&fixture, "0.1.0").unwrap();
    fixture.publish(&["release/0.1", "v0.1.0"]);

    assert_eq!(
        finalized(&fixture, "0.1.0"),
        Err("0.1.0 is finalized already: origin has its tag v0.1.0.".to_owned())
    );
}

#[test]
fn a_release_that_has_not_started_is_not_finalized() {
    let fixture = cut_0_1_0();

    assert_eq!(
        finalized(&fixture, "0.2.0"),
        Err("There is no release/0.2, so 0.2.0 hasn't been cut.".to_owned())
    );
    assert_eq!(fixture.git(&["tag", "--list"]), "");
}

#[test]
fn a_tag_elsewhere_is_left_alone() {
    let fixture = cut_0_1_0();
    fixture.git(&["tag", "v0.1.0", "main"]);
    let main = fixture.hash("main");
    let tip = fixture.hash("release/0.1");

    assert_eq!(
        finalized(&fixture, "0.1.0"),
        Err(format!(
            "This repository has a tag v0.1.0 at {}, not at the tip of release/0.1, {}; delete it \
             with `git tag -d v0.1.0`.",
            &main[..7],
            &tip[..7]
        ))
    );
    assert!(fixture.origin_tags().is_empty());
}

/// The tag holds the notes as they are when it is made, so they have to be
/// final: on `main`, where the notes branch merges.
#[test]
fn notes_not_merged_yet_are_not_tagged() {
    let fixture = cut_0_1_0();
    let head = fixture.hash("HEAD");

    assert_eq!(
        finalized(&fixture, "0.1.0"),
        Err(format!(
            "main at {} lacks 1 file of the notes of 0.1.0 that release-notes/v0.1.0 has: \
             changelog.d/0.1.0/retry.fixed.md. Merge release-notes/v0.1.0 into main first, then \
             finalize from there. If main dropped it on purpose, delete the branch with `git \
             branch -D release-notes/v0.1.0`.",
            &head[..7]
        ))
    );
    assert_eq!(fixture.git(&["tag", "--list"]), "");
}

/// A notes branch pushed from elsewhere counts as much as one made here.
#[test]
fn notes_on_origin_not_merged_yet_are_not_tagged() {
    let fixture = cut_0_1_0();
    fixture.publish(&["release/0.1", "release-notes/v0.1.0"]);
    fixture.git(&["branch", "-D", "release-notes/v0.1.0"]);
    let head = fixture.hash("HEAD");

    assert_eq!(
        finalized(&fixture, "0.1.0"),
        Err(format!(
            "main at {} lacks 1 file of the notes of 0.1.0 that origin's release-notes/v0.1.0 \
             has: changelog.d/0.1.0/retry.fixed.md. Merge release-notes/v0.1.0 into main first, \
             then finalize from there. If main dropped it on purpose, delete the branch with `git \
             push origin --delete release-notes/v0.1.0`.",
            &head[..7]
        ))
    );
}

/// Without its notes branch in sight, such as on another machine than the one
/// that cut it, a release's unfiled notes show as fragments it ships still at
/// the top level.
#[test]
fn fragments_of_the_release_at_the_top_level_are_not_tagged() {
    let fixture = cut_0_1_0();
    fixture.git(&["branch", "-D", "release-notes/v0.1.0"]);
    let head = fixture.hash("HEAD");

    assert_eq!(
        finalized(&fixture, "0.1.0"),
        Err(format!(
            "main at {} still has 1 file at the top of changelog.d whose changes release/0.1 \
             ships: changelog.d/retry.fixed.md. Their notes aren't filed yet. Merge \
             release-notes/v0.1.0 into main first, then finalize from there.",
            &head[..7]
        ))
    );
}

/// A patch release whose picks added no fragment has nothing for its tag to
/// say.
#[test]
fn a_release_without_notes_is_not_tagged() {
    let fixture = filed_0_1_0();
    finalized(&fixture, "0.1.0").unwrap();
    fixture.publish(&["release/0.1", "v0.1.0"]);
    fixture.write("README.md", "Tidied.\n");
    let tidy = fixture.commit("chore: Tidy");
    fixture.push();
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options).unwrap();
    pick(&pipeline, &version("0.1.1"), &[tidy]).unwrap();
    let head = fixture.hash("HEAD");

    assert_eq!(
        finalized(&fixture, "0.1.1"),
        Err(format!(
            "main at {} has no notes of 0.1.1 in changelog.d/0.1.1/. Write at least one fragment \
             there, such as changelog.d/0.1.1/<slug>.fixed.md, and finalize from a commit that \
             has it.",
            &head[..7]
        ))
    );
}

/// A rehearsal merges the notes branch into a branch of its own, and finalizes
/// from there.
#[test]
fn the_notes_come_from_the_commit_checked_out() {
    let fixture = cut_0_1_0();
    fixture.git(&["switch", "--quiet", "--create", "try-release"]);
    fixture.git(&["merge", "--quiet", "--no-edit", "release-notes/v0.1.0"]);
    let head = fixture.hash("HEAD");
    let main = fixture.hash("origin/main");

    let said = finalized(&fixture, "0.1.0").unwrap();

    assert!(
        said.contains(&format!(
            "with the notes of 0.1.0 on try-release at {}.\ntry-release at {} is ahead of \
             origin's main at {}, so the tag's notes may not match origin's main.\n",
            &head[..7],
            &head[..7],
            &main[..7]
        )),
        "{said}"
    );
    assert!(tag_message(&fixture, "v0.1.0").ends_with("- Retried.\n  (#1)"));
}
