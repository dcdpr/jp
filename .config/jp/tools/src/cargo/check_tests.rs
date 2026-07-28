use camino_tempfile::{Utf8TempDir, tempdir};
use jp_tool::{Action, Outcome};
use pretty_assertions::assert_eq;

use super::*;
use crate::util::runner::{ExitCode, MockProcessRunner, ProcessOutput};

fn ctx() -> (Utf8TempDir, Context) {
    let dir = tempdir().unwrap();
    let ctx = Context {
        root: dir.path().to_owned(),
        action: Action::Run,
        access: None,
        workspace_id: "test".into(),
        conversation_id: "test".into(),
    };

    (dir, ctx)
}

#[test]
fn test_cargo_check_with_warnings() {
    let (_dir, ctx) = ctx();

    let stderr = indoc::formatdoc! {r#"
            warning: unused `std::result::Result` that must be used
             --> src/main.rs:2:5
              |
            2 |     std::env::var("FOO");
              |     ^^^^^^^^^^^^^^^^^^^^
              |
              = note: this `Result` may be an `Err` variant, which should be handled
              = note: `#[warn(unused_must_use)]` (part of `#[warn(unused)]`) on by default
            help: use `let _ = ...` to ignore the resulting value
              |
            2 |     let _ = std::env::var("FOO");
              |     +++++++
            "#};

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns(ProcessOutput {
            stdout: String::new(),
            stderr,
            status: ExitCode::success(),
        })
        .expect("comfort")
        .returns_success("");

    let result = cargo_check_impl(&ctx, None, false, false, &runner).unwrap();

    assert_eq!(result.into_content().unwrap(), indoc::indoc! {r#"
            ```
            warning: unused `std::result::Result` that must be used
             --> src/main.rs:2:5
              |
            2 |     std::env::var("FOO");
              |     ^^^^^^^^^^^^^^^^^^^^
              |
              = note: this `Result` may be an `Err` variant, which should be handled
              = note: `#[warn(unused_must_use)]` (part of `#[warn(unused)]`) on by default
            help: use `let _ = ...` to ignore the resulting value
              |
            2 |     let _ = std::env::var("FOO");
              |     +++++++
            ```
        "#});
}

#[test]
fn test_cargo_check_no_warnings() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns_success("");

    let result = cargo_check_impl(&ctx, None, false, false, &runner).unwrap();

    assert_eq!(
        result.into_content().unwrap(),
        "Check succeeded. No warnings or errors found."
    );
}

#[test]
fn clean_clippy_with_comfort_drift_appends_note() {
    let (_dir, ctx) = ctx();
    let comfort_stdout = format!("{root}/src/lib.rs\n{root}/src/main.rs", root = ctx.root);

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns(ProcessOutput {
            stdout: comfort_stdout,
            stderr: String::new(),
            status: ExitCode::from_code(1),
        });

    let result = cargo_check_impl(&ctx, None, false, false, &runner).unwrap();

    assert_eq!(result.into_content().unwrap(), indoc::indoc! {"
            Check succeeded. No warnings or errors found.

            Doc comments in the following files are badly formatted. Run `cargo_fmt` to auto-fix them:
            - src/lib.rs
            - src/main.rs"});
}

#[test]
fn clippy_warnings_and_comfort_drift_are_both_reported() {
    let (_dir, ctx) = ctx();
    let comfort_stdout = format!("{root}/src/lib.rs", root = ctx.root);

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns(ProcessOutput {
            stdout: String::new(),
            stderr: "warning: something".to_owned(),
            status: ExitCode::success(),
        })
        .expect("comfort")
        .returns(ProcessOutput {
            stdout: comfort_stdout,
            stderr: String::new(),
            status: ExitCode::from_code(1),
        });

    let result = cargo_check_impl(&ctx, None, false, false, &runner).unwrap();

    assert_eq!(result.into_content().unwrap(), indoc::indoc! {"
            ```
            warning: something
            ```

            Doc comments in the following files are badly formatted. Run `cargo_fmt` to auto-fix them:
            - src/lib.rs"});
}

#[test]
fn comfort_drift_listing_is_bounded() {
    let (_dir, ctx) = ctx();
    let comfort_stdout = (0..4_000)
        .map(|i| format!("{root}/src/generated/file_{i}.rs", root = ctx.root))
        .collect::<Vec<_>>()
        .join("\n");

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns(ProcessOutput {
            stdout: comfort_stdout,
            stderr: String::new(),
            status: ExitCode::from_code(1),
        });

    let content = cargo_check_impl(&ctx, None, false, false, &runner)
        .unwrap()
        .unwrap_content();

    assert!(
        content.len() < MAX_DIAGNOSTIC_BYTES + 200,
        "drift note grew to {} bytes",
        content.len()
    );
    assert!(
        content.contains("[Truncated: showing"),
        "got tail: {}",
        &content[content.len() - 100..]
    );
}

#[test]
fn comfort_real_failure_is_reported_as_error() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns(ProcessOutput {
            stdout: String::new(),
            stderr: "comfort: parse error".to_owned(),
            status: ExitCode::from_code(2),
        });

    let result = cargo_check_impl(&ctx, None, false, false, &runner).unwrap();
    match result {
        Outcome::Error { message, .. } => {
            assert_eq!(message, "comfort failed: comfort: parse error");
        }
        _ => panic!("Expected Outcome::Error, got: {result:?}"),
    }
}

#[test]
fn clippy_failure_short_circuits_before_running_comfort() {
    let (_dir, ctx) = ctx();
    // Single expectation: comfort should never be reached.
    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns(ProcessOutput {
            stdout: String::new(),
            stderr: "error: build failed".to_owned(),
            status: ExitCode::from_code(101),
        });

    let result = cargo_check_impl(&ctx, None, false, false, &runner).unwrap();
    match result {
        Outcome::Error { message, .. } => {
            assert_eq!(message, "Cargo command failed: error: build failed");
        }
        _ => panic!("Expected Outcome::Error, got: {result:?}"),
    }
}

#[test]
fn package_scope_is_passed_through_to_both_tools() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .args(&[
            "clippy",
            "--color=never",
            "--package=my_pkg",
            "--quiet",
            "--all-targets",
        ])
        .returns_success("")
        .expect("comfort")
        .args(&[
            "--check",
            "--list-changed",
            "--format-markdown",
            "--reference-links",
            "--prune-reference-links",
            "--language",
            "rust",
            "--package",
            "my_pkg",
        ])
        .returns_success("");

    let result = cargo_check_impl(&ctx, Some("my_pkg"), false, false, &runner).unwrap();
    assert_eq!(
        result.into_content().unwrap(),
        "Check succeeded. No warnings or errors found."
    );
}

/// Rustdoc lints are denied on CI but invisible to clippy, so a clean clippy
/// run must still surface them.
#[test]
fn doc_lints_are_reported_alongside_a_clean_clippy_run() {
    let (_dir, ctx) = ctx();

    let doc_stderr = indoc::indoc! {"
            error: public documentation for `estimate_overhead_chars` links to private item `OVERHEAD_FACTOR`
              --> crates/jp_llm/src/window.rs:42:35
            error: could not document `jp_llm`
        "};

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("cargo")
        .returns(ProcessOutput {
            stdout: String::new(),
            stderr: doc_stderr.to_owned(),
            status: ExitCode::from_code(101),
        })
        .expect("comfort")
        .returns_success("");

    let result = cargo_check_impl(&ctx, None, false, true, &runner).unwrap();

    assert_eq!(result.into_content().unwrap(), indoc::indoc! {"
            Check succeeded. No warnings or errors found.

            Documentation lints failed. These are denied on CI (`just docs-ci`) and are not reported by clippy:

            ```
            error: public documentation for `estimate_overhead_chars` links to private item `OVERHEAD_FACTOR`
              --> crates/jp_llm/src/window.rs:42:35
            error: could not document `jp_llm`
            ```"});
}

#[test]
fn a_clean_doc_run_adds_nothing_to_the_output() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns_success("");

    let result = cargo_check_impl(&ctx, None, false, true, &runner).unwrap();

    assert_eq!(
        result.into_content().unwrap(),
        "Check succeeded. No warnings or errors found."
    );
}

/// Doc lints come before the comfort note: the first fails CI, the second is
/// auto-fixable.
#[test]
fn doc_lints_and_comfort_drift_are_both_reported() {
    let (_dir, ctx) = ctx();
    let comfort_stdout = format!("{root}/src/lib.rs", root = ctx.root);

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("cargo")
        .returns(ProcessOutput {
            stdout: String::new(),
            stderr: "error: unresolved link to `Nope`".to_owned(),
            status: ExitCode::from_code(101),
        })
        .expect("comfort")
        .returns(ProcessOutput {
            stdout: comfort_stdout,
            stderr: String::new(),
            status: ExitCode::from_code(1),
        });

    let result = cargo_check_impl(&ctx, None, false, true, &runner).unwrap();

    assert_eq!(result.into_content().unwrap(), indoc::indoc! {"
            Check succeeded. No warnings or errors found.

            Documentation lints failed. These are denied on CI (`just docs-ci`) and are not reported by clippy:

            ```
            error: unresolved link to `Nope`
            ```

            Doc comments in the following files are badly formatted. Run `cargo_fmt` to auto-fix them:
            - src/lib.rs"});
}

/// `--document-private-items` is what makes `private-intra-doc-links` fire, the
/// package scope has to reach rustdoc too, and `--profile=docs` keeps the run
/// on its own build lock instead of contending with the dev profile.
#[test]
fn doc_run_denies_the_ci_lints_and_honours_the_package_scope() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .args(&[
            "clippy",
            "--color=never",
            "--package=my_pkg",
            "--quiet",
            "--all-targets",
        ])
        .returns_success("")
        .expect("cargo")
        .args(&[
            "doc",
            "--color=never",
            "--package=my_pkg",
            "--quiet",
            "--profile=docs",
            "--no-deps",
            "--document-private-items",
            "--keep-going",
        ])
        .returns_success("")
        .expect("comfort")
        .returns_success("");

    let result = cargo_check_impl(&ctx, Some("my_pkg"), false, true, &runner).unwrap();
    assert_eq!(
        result.into_content().unwrap(),
        "Check succeeded. No warnings or errors found."
    );

    assert!(
        RUSTDOC_LINTS.contains(&"-D rustdoc::private-intra-doc-links"),
        "the lint that catches a public doc linking to a private item must be denied"
    );
}

/// A `cargo doc` failure with no diagnostics still has to say something.
#[test]
fn a_silent_doc_failure_reports_its_status() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("cargo")
        .returns(ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            status: ExitCode::from_code(101),
        })
        .expect("comfort")
        .returns_success("");

    let content = cargo_check_impl(&ctx, None, false, true, &runner)
        .unwrap()
        .unwrap_content();

    assert!(
        content.contains("`cargo doc` failed with exit status 101"),
        "got: {content}"
    );
}

/// Disabling the doc pass must not leave a stray `cargo doc` invocation behind:
/// `MockProcessRunner` panics on drop if an expectation goes unused, so the
/// two-expectation setup here is the assertion.
#[test]
fn docs_disabled_skips_the_doc_run() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns_success("");

    let result = cargo_check_impl(&ctx, None, false, false, &runner).unwrap();
    assert_eq!(
        result.into_content().unwrap(),
        "Check succeeded. No warnings or errors found."
    );
}
