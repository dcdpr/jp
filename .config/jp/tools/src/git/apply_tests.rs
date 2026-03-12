use super::*;
use crate::util::runner::MockProcessRunner;

#[test]
fn succeeds_first_try() {
    let runner = MockProcessRunner::builder()
        .expect("git")
        .args(&["apply", "--cached", "--unidiff-zero", "-"])
        .returns_success("");

    let result = apply_patch_to_index("some patch", "/tmp".into(), &runner);
    assert!(result.is_ok());
}

#[test]
fn non_lock_error_fails_immediately() {
    let runner = MockProcessRunner::builder()
        .expect("git")
        .args(&["apply", "--cached", "--unidiff-zero", "-"])
        .returns_error("error: patch does not apply");

    let result = apply_patch_to_index("bad patch", "/tmp".into(), &runner);
    assert!(result.unwrap_err().contains("patch does not apply"));
}

#[test]
fn retries_on_lock_contention() {
    let runner = MockProcessRunner::builder()
        .expect("git")
        .args(&["apply", "--cached", "--unidiff-zero", "-"])
        .returns_error(
            "fatal: Unable to create '/repo/.git/index.lock': File exists.\nAnother git process \
             seems to be running in this repository",
        )
        .expect("git")
        .args(&["apply", "--cached", "--unidiff-zero", "-"])
        .returns_success("");

    let result = apply_patch_to_index("some patch", "/tmp".into(), &runner);
    assert!(result.is_ok());
}
