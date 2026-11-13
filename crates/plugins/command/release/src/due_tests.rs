use serde_json::{Value, json};

use super::*;
use crate::{cut::cut, finalize::finalize, options::Options, repository_testing::Fixture};

/// 2026-10-01 12:00:00 UTC in seconds since the Unix epoch.
const OCTOBER_1: i64 = 1_790_856_000;
const TWO_WEEKS: i64 = 14 * 86_400;

fn version(text: &str) -> Version {
    Version::parse(text).unwrap()
}

fn options(settings: &Value) -> Options {
    let mut options = json!({ "forge": "none" });
    if let (Value::Object(options), Value::Object(settings)) = (&mut options, settings) {
        options.extend(settings.clone());
    }
    let Value::Object(options) = options else {
        panic!("options are an object");
    };

    Options::read(&options).unwrap()
}

fn due_at(fixture: &Fixture, now: i64, settings: &Value) -> Due {
    let shell = fixture.shell();
    let options = options(settings);
    let pipeline = Pipeline::new(&shell, fixture.root(), &options).unwrap();

    due(&pipeline, now).unwrap()
}

fn cut_on_october_1() -> Fixture {
    let fixture = Fixture::new();
    fixture.fragment("retry", "fixed", "Retried.");
    fixture.commit("fix: Retry (#1)");
    fixture.push();
    let shell = fixture.shell_at(OCTOBER_1);
    let options = options(&json!({}));
    let pipeline = Pipeline::new(&shell, fixture.root(), &options).unwrap();
    cut(&pipeline, None, None).unwrap();

    fixture
}

fn finalize_0_1_0(fixture: &Fixture) {
    fixture.merge_notes("0.1.0", "chore(release): Cut release 0.1.0 (#3)");
    let shell = fixture.shell();
    let options = options(&json!({}));
    let pipeline = Pipeline::new(&shell, fixture.root(), &options).unwrap();

    finalize(&pipeline, &version("0.1.0")).unwrap();
}

fn merge_a_fix(fixture: &Fixture) {
    fixture.fragment("crash", "fixed", "No more crash.");
    fixture.commit("fix: Stop a crash (#2)");
    fixture.push();
}

#[test]
fn the_next_release_is_due_a_cadence_after_the_last_was_cut() {
    let cadence = Options::default().cadence;

    assert_eq!(
        assess(
            &version("0.3.0"),
            OCTOBER_1,
            OCTOBER_1 + TWO_WEEKS,
            &cadence
        ),
        Due {
            due: true,
            reason: "0.3.0 was cut on 2026-10-01 and has been finalized, and the cadence is 2w, \
                     so the next release is due."
                .to_owned(),
        }
    );
    assert_eq!(
        assess(
            &version("0.3.0"),
            OCTOBER_1,
            OCTOBER_1 + TWO_WEEKS - 1,
            &cadence
        ),
        Due {
            due: false,
            reason: "0.3.0 was cut on 2026-10-01, and the cadence is 2w, so the next release is \
                     due at 2026-10-15 12:00 UTC."
                .to_owned(),
        }
    );
}

#[test]
fn the_first_release_is_due_before_any_is_cut() {
    let fixture = Fixture::new();
    merge_a_fix(&fixture);

    assert_eq!(due_at(&fixture, OCTOBER_1, &json!({})), Due {
        due: true,
        reason: "No release has been cut yet, so the first one is due.".to_owned(),
    });
}

/// A release the cadence says is due waits for something to release: `cut`
/// refuses to start one without an unreleased fragment.
#[test]
fn nothing_is_due_without_an_unreleased_fragment() {
    let fresh = Fixture::new();
    assert_eq!(due_at(&fresh, OCTOBER_1, &json!({})), Due {
        due: false,
        reason: "`main` has no unreleased fragments, so there is nothing to release yet; the \
                 first release is due once a fragment is merged."
            .to_owned(),
    });

    // Release 0.1.0 ships the change behind `main`'s only fragment, which its
    // notes moved into its directory.
    let released = cut_on_october_1();
    finalize_0_1_0(&released);
    assert_eq!(due_at(&released, OCTOBER_1 + TWO_WEEKS, &json!({})), Due {
        due: false,
        reason: "`main` has no unreleased fragments, so there is nothing to release yet; the next \
                 release is due once a fragment is merged."
            .to_owned(),
    });

    merge_a_fix(&released);
    assert!(due_at(&released, OCTOBER_1 + TWO_WEEKS, &json!({})).due);
}

/// However long ago 0.1.0 was cut, the next release waits for it.
#[test]
fn the_next_release_waits_for_the_one_in_preparation() {
    let fixture = cut_on_october_1();

    assert_eq!(
        due_at(&fixture, OCTOBER_1 + 10 * TWO_WEEKS, &json!({})),
        Due {
            due: false,
            reason: "0.1.0 is in preparation, so the next release is due once it is finalized."
                .to_owned(),
        }
    );
}

/// The cadence counts from the cut, not from the tag that finalized it.
#[test]
fn a_finalized_release_counts_from_its_cut() {
    let fixture = cut_on_october_1();
    finalize_0_1_0(&fixture);
    merge_a_fix(&fixture);

    assert_eq!(
        due_at(&fixture, OCTOBER_1 + TWO_WEEKS - 1, &json!({})),
        Due {
            due: false,
            reason: "0.1.0 was cut on 2026-10-01, and the cadence is 2w, so the next release is \
                     due at 2026-10-15 12:00 UTC."
                .to_owned(),
        }
    );
    assert!(due_at(&fixture, OCTOBER_1 + TWO_WEEKS, &json!({})).due);
}

#[test]
fn the_configured_cadence_decides() {
    let fixture = cut_on_october_1();
    finalize_0_1_0(&fixture);
    merge_a_fix(&fixture);
    let daily = json!({ "cadence": "1d" });

    assert!(!due_at(&fixture, OCTOBER_1 + 86_399, &daily).due);
    assert_eq!(
        due_at(&fixture, OCTOBER_1 + 86_400, &daily).reason,
        "0.1.0 was cut on 2026-10-01 and has been finalized, and the cadence is 1d, so the next \
         release is due."
    );
}

/// JP tagged v0.1.0 by hand, before the pipeline existed: no commit on a
/// release branch starts it, so it counts from its tag.
#[test]
fn a_release_tagged_by_hand_counts_from_its_tag() {
    let fixture = Fixture::new();
    fixture.git_at(OCTOBER_1, &[
        "tag",
        "--annotate",
        "v0.1.0",
        "--message",
        "Release 0.1.0",
    ]);
    fixture.git(&["push", "--quiet", "origin", "v0.1.0"]);
    merge_a_fix(&fixture);

    assert!(!due_at(&fixture, OCTOBER_1 + TWO_WEEKS - 1, &json!({})).due);
    assert_eq!(
        due_at(&fixture, OCTOBER_1 + TWO_WEEKS, &json!({})).reason,
        "0.1.0 was cut on 2026-10-01 and has been finalized, and the cadence is 2w, so the next \
         release is due."
    );
}
