use camino::Utf8Path;

use super::*;
use crate::util::runner::MockProcessRunner;

fn root() -> &'static Utf8Path {
    Utf8Path::new("/tmp")
}

fn bin() -> &'static Utf8Path {
    Utf8Path::new("/tmp/jpdrive")
}

#[test]
fn captures_what_the_driver_reports() {
    let runner = MockProcessRunner::builder()
        .expect("/tmp/jpdrive")
        .args(&["frontmost"])
        .returns_success(r#"{"bundle_id":"com.apple.Terminal"}"#)
        .expect("/tmp/jpdrive")
        .args(&["pointer"])
        .returns_success(r#"{"x":412.5,"y":88}"#);

    assert_eq!(capture(bin(), root(), &runner), Borrowed {
        frontmost: Some("com.apple.Terminal".to_owned()),
        pointer: Some((412.5, 88.0)),
    });
}

/// A driver that cannot say what is in front leaves focus alone afterwards.
/// Refusing to drive over it would be worse: the steps are what the caller
/// asked for, and this is housekeeping around them.
#[test]
fn captures_nothing_when_the_driver_reports_nothing() {
    let runner = MockProcessRunner::success("not json at all");

    assert_eq!(capture(bin(), root(), &runner), Borrowed::default());
}

#[test]
fn restores_focus_and_the_pointer() {
    let runner = MockProcessRunner::builder()
        .expect("/tmp/jpdrive")
        .args(&["frontmost", "--set", "com.apple.Terminal"])
        .returns_success("{}")
        .expect("/tmp/jpdrive")
        .args(&["pointer", "--set", "412.5,88"])
        .returns_success("{}");

    restore(
        &Borrowed {
            frontmost: Some("com.apple.Terminal".to_owned()),
            pointer: Some((412.5, 88.0)),
        },
        bin(),
        root(),
        &runner,
    );
}

/// Nothing borrowed, nothing put back: a run that captured no pointer must not
/// warp the cursor to a coordinate it invented.
///
/// Asserted by expecting the one call that is owed and nothing else.
/// The mock fails on an unfulfilled expectation, so the frontmost call has to
/// happen, and it has no expectation to match a pointer call — which is what
/// makes the absence of one an assertion rather than an omission.
#[test]
fn restores_only_what_it_captured() {
    let runner = MockProcessRunner::builder()
        .expect("/tmp/jpdrive")
        .args(&["frontmost", "--set", "com.apple.Terminal"])
        .returns_success("{}");

    restore(
        &Borrowed {
            frontmost: Some("com.apple.Terminal".to_owned()),
            pointer: None,
        },
        bin(),
        root(),
        &runner,
    );
}
