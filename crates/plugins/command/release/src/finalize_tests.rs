use serde_json::{Value, json};

use super::*;
use crate::{cut::cut, options::Options, repository_testing::Fixture};

fn options() -> Options {
    let Value::Object(options) = json!({ "forge": "none" }) else {
        panic!("options are an object");
    };

    Options::read(&options).unwrap()
}

fn version(text: &str) -> Version {
    Version::parse(text).unwrap()
}

/// A workspace whose release 0.1.0 was cut here, and not yet published.
fn cut_0_1_0() -> Fixture {
    let fixture = Fixture::new();
    fixture.fragment("retry", "fixed", "Retried.");
    fixture.commit("fix: Retry (#1)");
    fixture.push();
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options).unwrap();
    cut(&pipeline, None).unwrap();

    fixture
}

#[test]
fn finalize_tags_the_tip_of_the_release_branch_here_and_pushes_nothing() {
    let fixture = cut_0_1_0();
    fixture.publish(&["release/0.1"]);
    let tip = fixture.hash("release/0.1");
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options).unwrap();

    let said = finalize(&pipeline, &version("0.1.0")).unwrap();

    assert_eq!(
        said,
        format!(
            "Tagged the tip of release/0.1, {}, as v0.1.0.\n\nNothing is published yet. Push the \
             tag to release it:\n\n    git push origin v0.1.0\n",
            &tip[..7]
        )
    );
    assert_eq!(fixture.git(&["cat-file", "-t", "v0.1.0"]).trim(), "tag");
    assert_eq!(fixture.hash("v0.1.0^{commit}"), tip);
    assert_eq!(
        fixture
            .git(&["tag", "--list", "--format=%(contents)", "v0.1.0"])
            .trim(),
        "Release 0.1.0"
    );
    assert!(fixture.origin_tags().is_empty());
}

/// CI runs on what is pushed to a release branch, so a tag on commits `origin`
/// lacks is one CI hasn't checked.
#[test]
fn an_unpushed_release_branch_is_pushed_with_the_tag() {
    let fixture = cut_0_1_0();
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options).unwrap();

    let said = finalize(&pipeline, &version("0.1.0")).unwrap();

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
    let fixture = cut_0_1_0();
    fixture.publish(&["release/0.1"]);
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options).unwrap();
    finalize(&pipeline, &version("0.1.0")).unwrap();

    assert_eq!(
        finalize(&pipeline, &version("0.1.0")),
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
    let fixture = cut_0_1_0();
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options).unwrap();
    finalize(&pipeline, &version("0.1.0")).unwrap();
    fixture.publish(&["release/0.1", "v0.1.0"]);

    assert_eq!(
        finalize(&pipeline, &version("0.1.0")),
        Err("0.1.0 is finalized already: origin has its tag v0.1.0.".to_owned())
    );
}

#[test]
fn a_release_that_has_not_started_is_not_finalized() {
    let fixture = cut_0_1_0();
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options).unwrap();

    assert_eq!(
        finalize(&pipeline, &version("0.2.0")),
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
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options).unwrap();

    assert_eq!(
        finalize(&pipeline, &version("0.1.0")),
        Err(format!(
            "This repository has a tag v0.1.0 at {}, not at the tip of release/0.1, {}; delete it \
             with `git tag -d v0.1.0`.",
            &main[..7],
            &tip[..7]
        ))
    );
    assert!(fixture.origin_tags().is_empty());
}
