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

/// A workspace whose release 0.1.0 was cut.
fn cut_0_1_0() -> Fixture {
    let fixture = Fixture::new();
    fixture.fragment("retry", "fixed", "Retried.");
    fixture.commit("fix: Retry (#1)");
    fixture.push();
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options, None).unwrap();
    cut(&pipeline, None).unwrap();

    fixture
}

#[test]
fn finalize_tags_the_tip_of_the_release_branch_and_pushes_the_tag() {
    let fixture = cut_0_1_0();
    let tip = fixture
        .origin_git(&["rev-parse", "release/0.1"])
        .trim()
        .to_owned();
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options, None).unwrap();

    let said = finalize(&pipeline, &version("0.1.0")).unwrap();

    assert_eq!(
        said,
        format!(
            "Tagged the tip of release/0.1, {}, as v0.1.0, and pushed it.\n",
            &tip[..7]
        )
    );
    assert_eq!(
        fixture.origin_git(&["cat-file", "-t", "v0.1.0"]).trim(),
        "tag"
    );
    assert_eq!(
        fixture.origin_git(&["rev-parse", "v0.1.0^{commit}"]).trim(),
        tip
    );
    assert_eq!(
        fixture
            .origin_git(&["tag", "--list", "--format=%(contents)", "v0.1.0"])
            .trim(),
        "Release 0.1.0"
    );
}

#[test]
fn a_finalized_release_is_not_finalized_twice() {
    let fixture = cut_0_1_0();
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options, None).unwrap();
    finalize(&pipeline, &version("0.1.0")).unwrap();

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
    let pipeline = Pipeline::new(&shell, fixture.root(), &options, None).unwrap();

    assert_eq!(
        finalize(&pipeline, &version("0.2.0")),
        Err("There is no release/0.2, so 0.2.0 hasn't been cut.".to_owned())
    );
    assert_eq!(fixture.origin_git(&["tag", "--list"]), "");
}

/// A run that tagged and then failed to push finishes on the next run.
#[test]
fn a_tag_left_at_the_tip_is_pushed() {
    let fixture = cut_0_1_0();
    fixture.git(&["fetch", "--quiet", "origin"]);
    fixture.git(&[
        "tag",
        "--annotate",
        "--message",
        "Release 0.1.0",
        "v0.1.0",
        "origin/release/0.1",
    ]);
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options, None).unwrap();

    finalize(&pipeline, &version("0.1.0")).unwrap();

    assert_eq!(fixture.origin_git(&["tag", "--list"]), "v0.1.0\n");
}

#[test]
fn a_tag_elsewhere_is_left_alone() {
    let fixture = cut_0_1_0();
    fixture.git(&["tag", "v0.1.0", "main"]);
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options, None).unwrap();

    let error = finalize(&pipeline, &version("0.1.0")).unwrap_err();

    assert!(
        error.starts_with("This repository has a tag v0.1.0 at "),
        "{error}"
    );
    assert_eq!(fixture.origin_git(&["tag", "--list"]), "");
}
