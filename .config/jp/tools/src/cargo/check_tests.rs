use std::{env, fs};

use camino_tempfile::{Utf8TempDir, tempdir};
use jp_process::{ExitCode, MockProcessRunner, ProcessOutput, ProcessSpec};
use jp_tool::{Action, Context, Outcome};
use pretty_assertions::assert_eq;

use super::*;

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

    let result = cargo_check_impl(
        &ctx.root,
        "-W warnings",
        None,
        &[],
        Checks::default(),
        &runner,
    )
    .unwrap();

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

    let result = cargo_check_impl(
        &ctx.root,
        "-W warnings",
        None,
        &[],
        Checks::default(),
        &runner,
    )
    .unwrap();

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

    let result = cargo_check_impl(
        &ctx.root,
        "-W warnings",
        None,
        &[],
        Checks::default(),
        &runner,
    )
    .unwrap();

    // The header is clippy-scoped, not a blanket "Check succeeded", so it does
    // not contradict the drift note below it.
    assert_eq!(result.into_content().unwrap(), indoc::indoc! {"
            `cargo clippy` found no warnings or errors.

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

    let result = cargo_check_impl(
        &ctx.root,
        "-W warnings",
        None,
        &[],
        Checks::default(),
        &runner,
    )
    .unwrap();

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

    let content = cargo_check_impl(
        &ctx.root,
        "-W warnings",
        None,
        &[],
        Checks::default(),
        &runner,
    )
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

    let result = cargo_check_impl(
        &ctx.root,
        "-W warnings",
        None,
        &[],
        Checks::default(),
        &runner,
    )
    .unwrap();
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

    let result = cargo_check_impl(
        &ctx.root,
        "-W warnings",
        None,
        &[],
        Checks::default(),
        &runner,
    )
    .unwrap();
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
            "--keep-going",
            "--all-targets",
            "--all-features",
        ])
        .returns_success("")
        .expect("comfort")
        .args(&[
            "--check",
            "--list-changed",
            "--language",
            "rust",
            "--package",
            "my_pkg",
        ])
        .returns_success("");

    let result = cargo_check_impl(
        &ctx.root,
        "-W warnings",
        None,
        &["my_pkg".to_owned()],
        Checks::default(),
        &runner,
    )
    .unwrap();
    assert_eq!(
        result.into_content().unwrap(),
        "Check succeeded. No warnings or errors found."
    );
}

/// Several packages are checked in one clippy run and one comfort run, so
/// shared dependencies compile once.
#[test]
fn each_package_is_passed_through_to_both_tools() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .args(&[
            "clippy",
            "--color=never",
            "--package=jp_storage",
            "--package=jp_workspace",
            "--quiet",
            "--keep-going",
            "--all-targets",
            "--all-features",
        ])
        .returns_success("")
        .expect("comfort")
        .args(&[
            "--check",
            "--list-changed",
            "--language",
            "rust",
            "--package",
            "jp_storage",
            "--package",
            "jp_workspace",
        ])
        .returns_success("")
        .expect("jp")
        .args(&["--no-interactive", "wiki", "check"])
        .returns_success("Checked 12 pages: 0 errors, 0 warnings.\n");

    let result = cargo_check_impl(
        &ctx.root,
        "-W warnings",
        None,
        &["jp_storage".to_owned(), "jp_workspace".to_owned()],
        WIKI,
        &runner,
    )
    .unwrap();

    assert_eq!(
        result.into_content().unwrap(),
        "Check succeeded. No warnings or errors found."
    );
    // The wiki check covers the whole wiki, so it runs once however many
    // packages are named.
    assert_eq!(runner.calls().len(), 3);
}

/// The configured profile has to reach clippy.
///
/// Without it the run lands in whichever profile directory the developer's own
/// builds use, where it takes the build lock and rewrites the fingerprints they
/// are relying on.
#[test]
fn profile_is_passed_through_to_clippy() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .args(&[
            "clippy",
            "--color=never",
            "--workspace",
            "--quiet",
            "--keep-going",
            "--all-targets",
            "--all-features",
            "--profile=agent",
        ])
        .returns_success("")
        .expect("comfort")
        .returns_success("");

    let result = cargo_check_impl(
        &ctx.root,
        "-W warnings",
        Some("agent"),
        &[],
        Checks::default(),
        &runner,
    )
    .unwrap();

    assert_eq!(
        result.into_content().unwrap(),
        "Check succeeded. No warnings or errors found."
    );
}

/// Unconfigured means no flag at all, leaving the profile to cargo.
#[test]
fn clippy_gets_no_profile_flag_unless_configured() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .args(&[
            "clippy",
            "--color=never",
            "--workspace",
            "--quiet",
            "--keep-going",
            "--all-targets",
            "--all-features",
        ])
        .returns_success("")
        .expect("comfort")
        .returns_success("");

    let result = cargo_check_impl(
        &ctx.root,
        "-W warnings",
        None,
        &[],
        Checks::default(),
        &runner,
    )
    .unwrap();

    assert_eq!(
        result.into_content().unwrap(),
        "Check succeeded. No warnings or errors found."
    );
}

/// `CARGO_UNSTABLE_CHECKSUM_FRESHNESS` has to reach cargo.
///
/// Sibling git worktrees share a target directory, and mtime-based freshness
/// lets one checkout serve the other's stale artifacts; content checksums are
/// what prevent that.
#[test]
fn checksum_freshness_reaches_cargo() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns_success("");

    cargo_check_impl(
        &ctx.root,
        "-W warnings",
        None,
        &[],
        Checks {
            checksum_freshness: true,
            ..Checks::default()
        },
        &runner,
    )
    .unwrap();

    let call = call_with_arg(&runner, "clippy").expect("the clippy pass must have run");

    assert_eq!(
        call.env
            .iter()
            .find(|(key, _)| key == "CARGO_UNSTABLE_CHECKSUM_FRESHNESS")
            .map(|(_, value)| value.as_str()),
        Some("true"),
    );
}

/// Off unless opted into, so the tools also work on stable cargo.
#[test]
fn checksum_freshness_is_absent_unless_opted_into() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns_success("");

    cargo_check_impl(
        &ctx.root,
        "-W warnings",
        None,
        &[],
        Checks::default(),
        &runner,
    )
    .unwrap();

    let call = call_with_arg(&runner, "clippy").expect("the clippy pass must have run");

    assert!(
        !call
            .env
            .iter()
            .any(|(key, _)| key == "CARGO_UNSTABLE_CHECKSUM_FRESHNESS"),
        "the clippy pass must not require nightly cargo unless asked to",
    );
}

const WIKI: Checks = Checks {
    checksum_freshness: false,
    wiki: true,
    docs: false,
    docs_mergeable_info: false,
    qual: false,
    site: false,
    wasm: false,
};

/// A clean wiki adds nothing: the run reads as it would without the check.
#[test]
fn a_clean_wiki_adds_nothing() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns_success("")
        .expect("jp")
        .args(&["--no-interactive", "wiki", "check"])
        .returns_success("Checked 12 pages: 0 errors, 0 warnings.\n");

    let result = cargo_check_impl(&ctx.root, "-W warnings", None, &[], WIKI, &runner).unwrap();

    assert_eq!(
        result.into_content().unwrap(),
        "Check succeeded. No warnings or errors found."
    );
}

/// A wiki page broken by a code change is reported beside clippy's findings,
/// warnings included, which `jp wiki check` exits 0 on.
#[test]
fn wiki_findings_are_reported() {
    let (_dir, ctx) = ctx();
    let found = "error: docs/wiki/a.md:9: `../../x.rs#L2-L9`: lines 2 to 9 are past the \
                 file.\n\nChecked 1 page: 1 error, 0 warnings.\n";

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns_success("")
        .expect("jp")
        .returns(ProcessOutput {
            stdout: found.to_owned(),
            stderr: String::new(),
            status: ExitCode::from_code(1),
        });

    let result = cargo_check_impl(&ctx.root, "-W warnings", None, &[], WIKI, &runner).unwrap();

    assert_eq!(result.into_content().unwrap(), indoc::indoc! {"
            `cargo clippy` found no warnings or errors.

            `jp wiki check` found problems:
            ```
            error: docs/wiki/a.md:9: `../../x.rs#L2-L9`: lines 2 to 9 are past the file.

            Checked 1 page: 1 error, 0 warnings.
            ```"});
}

/// A wiki `jp` can't check is a note, not a failed run: the code was checked.
#[test]
fn a_wiki_that_cant_be_checked_is_a_note() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns_success("")
        .expect("jp")
        .returns(ProcessOutput {
            stdout: String::new(),
            stderr: "error: unknown command `wiki`".to_owned(),
            status: ExitCode::from_code(2),
        });

    let result = cargo_check_impl(&ctx.root, "-W warnings", None, &[], WIKI, &runner).unwrap();

    assert_eq!(result.into_content().unwrap(), indoc::indoc! {"
            `cargo clippy` found no warnings or errors.

            The wiki wasn't checked: `jp wiki check` failed: error: unknown command `wiki`"});
}

const QUAL: Checks = Checks {
    checksum_freshness: false,
    wiki: false,
    docs: false,
    docs_mergeable_info: false,
    qual: true,
    site: false,
    wasm: false,
};

/// A failing verdict as `qual diff --format json` prints it.
/// `qual`'s own tests assert that it prints exactly this file, so a change to
/// the shape of its output reaches the tests that read it here.
const QUAL_VERDICT: &str =
    include_str!("../../../../../crates/internal/qual/fixtures/verdict.json");

const QUAL_PASSING: &str =
    r#"{"new": [], "enforced": [], "base": {"main": 3}, "head": {"main": 3}, "rules": {}}"#;

/// One new finding in each of the packages `a` and `b`.
const QUAL_TWO_PACKAGES: &str = indoc::indoc! {r#"
    {
      "new": [
        {
          "file": "crates/a/src/lib.rs",
          "line": 7,
          "rule": "DRY-002",
          "symbol": "unused",
          "message": "dead code: unused",
          "pass": "dry"
        },
        {
          "file": "crates/b/src/lib.rs",
          "line": 3,
          "rule": "DRY-002",
          "symbol": "stale",
          "message": "dead code: stale",
          "pass": "dry"
        }
      ],
      "enforced": [],
      "base": { "dry": 0 },
      "head": { "dry": 2 },
      "rules": { "DRY-002": { "title": "Dead code detected" } }
    }
"#};

/// `cargo metadata` for a workspace at `/ws` holding the packages `a` and `b`.
const METADATA: &str = indoc::indoc! {r#"
    {
      "workspace_root": "/ws",
      "packages": [
        { "name": "a", "manifest_path": "/ws/crates/a/Cargo.toml" },
        { "name": "b", "manifest_path": "/ws/crates/b/Cargo.toml" }
      ]
    }
"#};

const REMOTE_MAIN: &[&str] = &["rev-parse", "--verify", "--quiet", "origin/main^{commit}"];

const QUAL_DIFF: &[&str] = &[
    "run",
    "--quiet",
    "--package",
    "qual",
    "--",
    "diff",
    "--base",
    "origin/main",
    "--format",
    "json",
];

/// A verdict printed with the exit code `qual` gives a failing one.
fn failing_verdict(stdout: &str) -> ProcessOutput {
    ProcessOutput {
        stdout: stdout.to_owned(),
        stderr: String::new(),
        status: ExitCode::from_code(1),
    }
}

/// A passing gate adds nothing: the run reads as it would without the gate.
#[test]
fn a_passing_gate_adds_nothing() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns_success("")
        .expect("git")
        .args(REMOTE_MAIN)
        .returns_success("0123abcd\n")
        .expect("cargo")
        .args(QUAL_DIFF)
        .returns_success(QUAL_PASSING);

    let result = cargo_check_impl(&ctx.root, "-W warnings", None, &[], QUAL, &runner).unwrap();

    assert_eq!(
        result.into_content().unwrap(),
        "Check succeeded. No warnings or errors found."
    );
}

#[test]
fn new_findings_are_listed_under_their_rule_with_its_guidance() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns_success("")
        .expect("git")
        .returns_success("0123abcd\n")
        .expect("cargo")
        .returns(failing_verdict(QUAL_VERDICT));

    let result = cargo_check_impl(&ctx.root, "-W warnings", None, &[], QUAL, &runner).unwrap();

    assert_eq!(result.into_content().unwrap(), indoc::indoc! {r#"
            `cargo clippy` found no warnings or errors.

            The `qual` job on CI fails on these rustqual findings:

            # New rustqual findings (2)

            ## CX-003

            - `src/a.rs:12` in `run`: Magic number 42 in run
              2 findings share this rule and symbol, 1 more than at the base.

            ## CX-004: Function length

            - `src/a.rs:40` in `parse`: Function length 72 in parse

            Return early.
            Extract a helper only when it has a name worth reading.

            # rustqual findings in enforced rules (1)

            ## DRY-004: Wildcard import

            - `src/b.rs:3`: wildcard import: std::io

            Fix each finding. Don't silence one with a marker, a changed threshold, or an edit under `.config/rustqual/` unless the user asked for that in this conversation. If a finding looks wrong, say so. The markers rustqual accepts:

            - `// qual:allow(<dimension>, <target>) reason: "<why>"` above an item suppresses that kind of finding in it.
            - `// qual:api` above a public item whose callers live outside the workspace.
            - `// qual:test_helper` above an item used only from tests."#});
}

/// `package` selects the findings listed, not the code analysed: a change in
/// one package can leave a function in another without callers, and CI fails on
/// that finding too.
#[test]
fn package_selects_the_findings_listed_and_counts_the_rest() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns_success("")
        .expect("git")
        .returns_success("0123abcd\n")
        .expect("cargo")
        .args(QUAL_DIFF)
        .returns(failing_verdict(QUAL_TWO_PACKAGES))
        .expect("cargo")
        .args(&["metadata", "--no-deps", "--format-version=1"])
        .returns_success(METADATA);

    let result = cargo_check_impl(
        &ctx.root,
        "-W warnings",
        None,
        &["a".to_owned()],
        QUAL,
        &runner,
    )
    .unwrap();

    assert_eq!(result.into_content().unwrap(), indoc::indoc! {r#"
            `cargo clippy` found no warnings or errors.

            The `qual` job on CI fails on these rustqual findings:

            # New rustqual findings (1)

            ## DRY-002: Dead code detected

            - `crates/a/src/lib.rs:7` in `unused`: dead code: unused

            Findings outside the checked packages: 1. Run `cargo_check` without `package` to list them.

            Fix each finding. Don't silence one with a marker, a changed threshold, or an edit under `.config/rustqual/` unless the user asked for that in this conversation. If a finding looks wrong, say so. The markers rustqual accepts:

            - `// qual:allow(<dimension>, <target>) reason: "<why>"` above an item suppresses that kind of finding in it.
            - `// qual:api` above a public item whose callers live outside the workspace.
            - `// qual:test_helper` above an item used only from tests."#});
}

#[test]
fn findings_only_outside_the_package_are_counted() {
    let (_dir, ctx) = ctx();
    let metadata = indoc::indoc! {r#"
        {
          "workspace_root": "/ws",
          "packages": [{ "name": "c", "manifest_path": "/ws/crates/c/Cargo.toml" }]
        }
    "#};

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns_success("")
        .expect("git")
        .returns_success("0123abcd\n")
        .expect("cargo")
        .returns(failing_verdict(QUAL_TWO_PACKAGES))
        .expect("cargo")
        .returns_success(metadata);

    let result = cargo_check_impl(
        &ctx.root,
        "-W warnings",
        None,
        &["c".to_owned()],
        QUAL,
        &runner,
    )
    .unwrap();

    assert_eq!(result.into_content().unwrap(), indoc::indoc! {"
            `cargo clippy` found no warnings or errors.

            The `qual` job on CI fails on rustqual findings outside the checked packages (2). Run `cargo_check` without `package` to list them."});
}

/// A checkout without the `origin` remote compares against its local `main`.
#[test]
fn the_local_main_is_the_base_without_a_remote_one() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns_success("")
        .expect("git")
        .args(REMOTE_MAIN)
        .returns_error("")
        .expect("cargo")
        .args(&[
            "run",
            "--quiet",
            "--package",
            "qual",
            "--",
            "diff",
            "--base",
            "main",
            "--format",
            "json",
        ])
        .returns_success(QUAL_PASSING);

    let result = cargo_check_impl(&ctx.root, "-W warnings", None, &[], QUAL, &runner).unwrap();

    assert_eq!(
        result.into_content().unwrap(),
        "Check succeeded. No warnings or errors found."
    );
}

/// The gate is built with the profile and flags of the clippy run, so the two
/// share a directory under `target/` and neither rebuilds what the other built.
#[test]
fn the_gate_is_built_with_the_profile_and_flags_of_the_clippy_run() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns_success("")
        .expect("git")
        .returns_success("0123abcd\n")
        .expect("cargo")
        .args(&[
            "run",
            "--quiet",
            "--package",
            "qual",
            "--profile=agent",
            "--",
            "diff",
            "--base",
            "origin/main",
            "--format",
            "json",
        ])
        .returns_success(QUAL_PASSING);

    cargo_check_impl(
        &ctx.root,
        "-W warnings -Zthreads=0",
        Some("agent"),
        &[],
        QUAL,
        &runner,
    )
    .unwrap();

    let call = call_with_arg(&runner, "run").expect("the gate must have run");
    assert_eq!(call.env, vec![(
        "RUSTFLAGS".to_owned(),
        "-W warnings -Zthreads=0".to_owned()
    )]);
}

/// A gate that reaches no verdict is a note, not a failed run: clippy's result
/// stands, and an error outcome would drop it.
#[test]
fn a_gate_that_reaches_no_verdict_is_a_note() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns(ProcessOutput {
            stdout: String::new(),
            stderr: "warning: something".to_owned(),
            status: ExitCode::success(),
        })
        .expect("comfort")
        .returns_success("")
        .expect("git")
        .returns_success("0123abcd\n")
        .expect("cargo")
        .returns(ProcessOutput {
            stdout: String::new(),
            stderr: "qual: could not run `rustqual --version`: No such file or directory\n"
                .to_owned(),
            status: ExitCode::from_code(2),
        });

    let result = cargo_check_impl(&ctx.root, "-W warnings", None, &[], QUAL, &runner).unwrap();

    assert_eq!(result.into_content().unwrap(), indoc::indoc! {"
            ```
            warning: something
            ```

            New rustqual findings weren't checked: `qual diff --base origin/main` failed: qual: could not run `rustqual --version`: No such file or directory"});
}

/// A verdict without a field the note is built from is not read as a pass.
#[test]
fn a_verdict_that_cant_be_read_is_a_note() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns_success("")
        .expect("git")
        .returns_success("0123abcd\n")
        .expect("cargo")
        .returns_success(r#"{"new": []}"#);

    let result = cargo_check_impl(&ctx.root, "-W warnings", None, &[], QUAL, &runner).unwrap();

    assert_eq!(result.into_content().unwrap(), indoc::indoc! {"
            `cargo clippy` found no warnings or errors.

            New rustqual findings weren't checked: `qual diff` printed a verdict that can't be read: missing field `enforced` at line 1 column 11"});
}

/// Findings in code that doesn't compile are noise, so the gate waits for
/// clippy to succeed.
#[test]
fn clippy_failure_short_circuits_before_running_the_gate() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns(ProcessOutput {
            stdout: String::new(),
            stderr: "error: build failed".to_owned(),
            status: ExitCode::from_code(101),
        });

    let result = cargo_check_impl(&ctx.root, "-W warnings", None, &[], QUAL, &runner).unwrap();

    match result {
        Outcome::Error { message, .. } => {
            assert_eq!(message, "Cargo command failed: error: build failed");
        }
        _ => panic!("Expected Outcome::Error, got: {result:?}"),
    }
    assert_eq!(runner.calls().len(), 1);
}

const SITE: Checks = Checks {
    checksum_freshness: false,
    wiki: false,
    docs: false,
    docs_mergeable_info: false,
    qual: false,
    site: true,
    wasm: false,
};

const WASM: Checks = Checks {
    checksum_freshness: false,
    wiki: false,
    docs: false,
    docs_mergeable_info: false,
    qual: false,
    site: false,
    wasm: true,
};

/// `cargo metadata` for a workspace holding the host crate `a` and the browser
/// crate `a-ui`.
const BROWSER_METADATA: &str = r#"{
  "workspace_root": "/ws",
  "packages": [
    { "name": "a", "manifest_path": "/ws/a/Cargo.toml", "metadata": null },
    { "name": "a-ui", "manifest_path": "/ws/a/ui/Cargo.toml", "metadata": { "jp": { "wasm": true } } }
  ]
}"#;

const WASM_CLIPPY: &[&str] = &[
    "clippy",
    "--color=never",
    "--quiet",
    "--keep-going",
    "--all-targets",
    "--target",
    "wasm32-unknown-unknown",
    "--package=a-ui",
];

/// The browser crates build clean for the browser too: nothing is added.
#[test]
fn a_clean_browser_build_adds_nothing() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns_success("")
        .expect("cargo")
        .args(&["metadata", "--no-deps", "--format-version=1"])
        .returns_success(BROWSER_METADATA)
        .expect("cargo")
        .args(WASM_CLIPPY)
        .returns_success("");

    let result = cargo_check_impl(&ctx.root, "-W warnings", None, &[], WASM, &runner).unwrap();

    assert_eq!(
        result.into_content().unwrap(),
        "Check succeeded. No warnings or errors found."
    );
}

#[test]
fn what_only_the_browser_build_finds_is_reported() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns_success("")
        .expect("cargo")
        .returns_success(BROWSER_METADATA)
        .expect("cargo")
        .args(WASM_CLIPPY)
        .returns(ProcessOutput {
            stdout: String::new(),
            stderr: "warning: unused import: `Element`\n".to_owned(),
            status: ExitCode::success(),
        });

    let result = cargo_check_impl(&ctx.root, "-W warnings", None, &[], WASM, &runner).unwrap();

    assert_eq!(result.into_content().unwrap(), indoc::indoc! {"
            `cargo clippy` found no warnings or errors.

            `cargo clippy --target wasm32-unknown-unknown` found, in the browser crates:
            ```
            warning: unused import: `Element`
            ```"});
}

/// A browser build that fails says so, apart from one that only warns.
#[test]
fn a_failed_browser_build_is_reported_as_failed() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns_success("")
        .expect("cargo")
        .returns_success(BROWSER_METADATA)
        .expect("cargo")
        .args(WASM_CLIPPY)
        .returns(ProcessOutput {
            stdout: String::new(),
            stderr: "error[E0425]: cannot find value `x`\n".to_owned(),
            status: ExitCode::from_code(101),
        });

    let result = cargo_check_impl(&ctx.root, "-W warnings", None, &[], WASM, &runner).unwrap();

    assert_eq!(result.into_content().unwrap(), indoc::indoc! {"
            `cargo clippy` found no warnings or errors.

            `cargo clippy --target wasm32-unknown-unknown` failed on the browser crates:
            ```
            error[E0425]: cannot find value `x`
            ```"});
}

#[test]
fn browser_crates_that_cant_be_found_are_noted_and_not_built() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns_success("")
        .expect("cargo")
        .returns_error("could not find `Cargo.toml`\n");

    let result = cargo_check_impl(&ctx.root, "-W warnings", None, &[], WASM, &runner).unwrap();

    assert_eq!(
        result.into_content().unwrap(),
        "`cargo clippy` found no warnings or errors.\n\nThe browser crates weren't checked for \
         wasm32-unknown-unknown: `cargo metadata` failed: could not find `Cargo.toml`"
    );
    assert_eq!(runner.calls().len(), 3);
}

/// Checking packages none of which is a browser crate builds nothing for the
/// browser.
#[test]
fn packages_without_a_browser_crate_build_nothing_for_it() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns_success("")
        .expect("cargo")
        .returns_success(BROWSER_METADATA);

    let result = cargo_check_impl(
        &ctx.root,
        "-W warnings",
        None,
        &["a".to_owned()],
        WASM,
        &runner,
    )
    .unwrap();

    assert_eq!(
        result.into_content().unwrap(),
        "Check succeeded. No warnings or errors found."
    );
    assert_eq!(runner.calls().len(), 3);
}

const SITE_TEST_ARGS: &[&str] = &[
    "test",
    "--quiet",
    "--package",
    "jp-vite",
    "--lib",
    "--",
    "--exact",
    "tests::jp_site_is_synced",
];

const SITE_PASSED: &str = indoc::indoc! {"

    running 1 test
    .
    test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 41 filtered out; finished in 0.05s

"};

const SITE_FAILED: &str = indoc::indoc! {"

    running 1 test
    F
    failures:

    ---- tests::jp_site_is_synced stdout ----

    thread 'tests::jp_site_is_synced' (2953) panicked at crates/plugins/command/vite/src/lib_tests.rs:651:5:
    JP's site isn't the one this `jp vite` writes:
      write   docs/.vitepress/jp-vite/settings.json
    Run `jp vite sync`, or `JP_VITE_BLESS=1 cargo test -p jp-vite jp_site_is_synced`.
    note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace


    failures:
        tests::jp_site_is_synced

    test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 41 filtered out; finished in 0.05s

"};

#[test]
fn a_synced_site_adds_nothing() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns_success("")
        .expect("cargo")
        .args(SITE_TEST_ARGS)
        .returns_success(SITE_PASSED);

    let result = cargo_check_impl(&ctx.root, "-W warnings", None, &[], SITE, &runner).unwrap();

    assert_eq!(
        result.into_content().unwrap(),
        "Check succeeded. No warnings or errors found."
    );
    assert_eq!(runner.calls().len(), 3);
}

#[test]
fn an_unsynced_site_is_reported() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns_success("")
        .expect("cargo")
        .args(SITE_TEST_ARGS)
        .returns(ProcessOutput {
            stdout: SITE_FAILED.to_owned(),
            stderr: "error: test failed, to rerun pass `-p jp-vite --lib`\n".to_owned(),
            status: ExitCode::from_code(101),
        });

    let result = cargo_check_impl(&ctx.root, "-W warnings", None, &[], SITE, &runner).unwrap();

    assert_eq!(result.into_content().unwrap(), indoc::indoc! {"
            `cargo clippy` found no warnings or errors.

            JP's site isn't synced, which the `docs` job on CI fails on:
            ```
            JP's site isn't the one this `jp vite` writes:
              write   docs/.vitepress/jp-vite/settings.json
            Run `jp vite sync`, or `JP_VITE_BLESS=1 cargo test -p jp-vite jp_site_is_synced`.
            ```
            Run `JP_VITE_BLESS=1 cargo test -p jp-vite jp_site_is_synced` to sync it, rather than editing the files under `docs/.vitepress/jp-vite/` by hand. `jp vite sync` writes the site files the installed `jp-vite` was built with, which can predate this checkout's."});
}

/// `--exact` with a filter that matches no test exits 0 after running 0 tests,
/// which must not be treated as a synced site.
#[test]
fn a_site_test_that_never_ran_is_a_note() {
    let (_dir, ctx) = ctx();
    let nothing_ran = indoc::indoc! {"

        running 0 tests

        test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 42 filtered out; finished in 0.00s

    "};

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns_success("")
        .expect("cargo")
        .returns_success(nothing_ran);

    let result = cargo_check_impl(&ctx.root, "-W warnings", None, &[], SITE, &runner).unwrap();

    assert_eq!(result.into_content().unwrap(), indoc::indoc! {"
            `cargo clippy` found no warnings or errors.

            JP's site wasn't checked: `jp-vite` has no test named `tests::jp_site_is_synced`."});
}

#[test]
fn a_site_test_that_doesnt_build_is_a_note() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns_success("")
        .expect("cargo")
        .returns(ProcessOutput {
            stdout: String::new(),
            stderr: "error: package ID specification `jp-vite` did not match any packages\n"
                .to_owned(),
            status: ExitCode::from_code(101),
        });

    let result = cargo_check_impl(&ctx.root, "-W warnings", None, &[], SITE, &runner).unwrap();

    assert_eq!(result.into_content().unwrap(), indoc::indoc! {"
            `cargo clippy` found no warnings or errors.

            JP's site wasn't checked: `cargo test -p jp-vite` failed: error: package ID specification `jp-vite` did not match any packages"});
}

#[test]
fn the_site_test_is_built_with_the_profile_and_flags_of_the_clippy_run() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns_success("")
        .expect("cargo")
        .args(&[
            "test",
            "--quiet",
            "--package",
            "jp-vite",
            "--lib",
            "--profile=agent",
            "--",
            "--exact",
            "tests::jp_site_is_synced",
        ])
        .returns_success(SITE_PASSED);

    cargo_check_impl(
        &ctx.root,
        "-W warnings -Zthreads=0",
        Some("agent"),
        &[],
        SITE,
        &runner,
    )
    .unwrap();

    let call = call_with_arg(&runner, "test").expect("the site test must have run");
    assert_eq!(call.env, vec![(
        "RUSTFLAGS".to_owned(),
        "-W warnings -Zthreads=0".to_owned()
    )]);
}

const DOCS: Checks = Checks {
    checksum_freshness: false,
    wiki: false,
    docs: true,
    docs_mergeable_info: false,
    qual: false,
    site: false,
    wasm: false,
};

const DOC_ARGS: &[&str] = &[
    "doc",
    "--color=never",
    "--quiet",
    "--workspace",
    "--keep-going",
    "--all-features",
    "--document-private-items",
    "--no-deps",
];

#[test]
fn clean_docs_add_nothing() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns_success("")
        .expect("cargo")
        .args(DOC_ARGS)
        .returns_success("");

    let result = cargo_check_impl(&ctx.root, "-W warnings", None, &[], DOCS, &runner).unwrap();

    assert_eq!(
        result.into_content().unwrap(),
        "Check succeeded. No warnings or errors found."
    );
    assert_eq!(runner.calls().len(), 3);
}

#[test]
fn rustdoc_errors_are_reported() {
    let (_dir, ctx) = ctx();
    let stderr = indoc::indoc! {"
        error: unresolved link to `layout::site`
          --> crates/plugins/command/review/ui/src/compose.rs:13:7
           |
        13 | //! [`layout::site`] says which, and [`at`] draws the composer there.
           |       ^^^^^^^^^^^^ no item named `layout` in scope
           |
           = note: `-D rustdoc::broken-intra-doc-links` implied by `-D warnings`

        error: could not document `jp-review-ui`
    "};

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns_success("")
        .expect("cargo")
        .args(DOC_ARGS)
        .returns(ProcessOutput {
            stdout: String::new(),
            stderr: stderr.to_owned(),
            status: ExitCode::from_code(101),
        });

    let result = cargo_check_impl(&ctx.root, "-W warnings", None, &[], DOCS, &runner).unwrap();

    assert_eq!(result.into_content().unwrap(), indoc::indoc! {"
            `cargo clippy` found no warnings or errors.

            `cargo doc`, run as the `docs` job on CI runs it, reported:
            ```
            error: unresolved link to `layout::site`
              --> crates/plugins/command/review/ui/src/compose.rs:13:7
               |
            13 | //! [`layout::site`] says which, and [`at`] draws the composer there.
               |       ^^^^^^^^^^^^ no item named `layout` in scope
               |
               = note: `-D rustdoc::broken-intra-doc-links` implied by `-D warnings`

            error: could not document `jp-review-ui`
            ```"});
}

/// A failing `cargo doc` that printed nothing has no finding to report, and
/// must not read as clean docs.
#[test]
fn a_cargo_doc_that_fails_silently_is_a_note() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns_success("")
        .expect("cargo")
        .args(DOC_ARGS)
        .returns(ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            status: ExitCode::from_code(101),
        });

    let result = cargo_check_impl(&ctx.root, "-W warnings", None, &[], DOCS, &runner).unwrap();

    assert_eq!(result.into_content().unwrap(), indoc::indoc! {"
            `cargo clippy` found no warnings or errors.

            The docs weren't checked: `cargo doc` failed without saying why."});
}

/// The doc pass shares the clippy run's profile and `RUSTFLAGS`, so it reuses
/// that run's target directory instead of building one of its own.
#[test]
fn docs_are_built_with_the_scope_profile_and_flags_of_the_clippy_run() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns_success("")
        .expect("cargo")
        .args(&[
            "doc",
            "--color=never",
            "--quiet",
            "--package=jp_storage",
            "--package=jp_workspace",
            "--keep-going",
            "--all-features",
            "--document-private-items",
            "--no-deps",
            "--profile=agent",
        ])
        .returns_success("");

    cargo_check_impl(
        &ctx.root,
        "-W warnings -Zthreads=0",
        Some("agent"),
        &["jp_storage".to_owned(), "jp_workspace".to_owned()],
        DOCS,
        &runner,
    )
    .unwrap();

    let call = call_with_arg(&runner, "doc").expect("the doc pass must have run");
    assert_eq!(call.env, vec![
        ("RUSTFLAGS".to_owned(), "-W warnings -Zthreads=0".to_owned()),
        ("RUSTDOCFLAGS".to_owned(), DOCS_RUSTDOCFLAGS.to_owned()),
    ]);
}

#[test]
fn mergeable_info_is_passed_to_the_doc_pass_when_enabled() {
    let (_dir, ctx) = ctx();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_success("")
        .expect("comfort")
        .returns_success("")
        .expect("cargo")
        .args(DOC_ARGS)
        .returns_success("");

    cargo_check_impl(
        &ctx.root,
        "-W warnings",
        None,
        &[],
        Checks {
            docs_mergeable_info: true,
            ..DOCS
        },
        &runner,
    )
    .unwrap();

    let call = call_with_arg(&runner, "doc").expect("the doc pass must have run");
    assert_eq!(call.env, vec![
        ("RUSTFLAGS".to_owned(), "-W warnings".to_owned()),
        ("RUSTDOCFLAGS".to_owned(), DOCS_RUSTDOCFLAGS.to_owned()),
        (
            "CARGO_UNSTABLE_RUSTDOC_MERGEABLE_INFO".to_owned(),
            "true".to_owned()
        ),
    ]);
}

/// Verify that `doc_check` uses the same flags as `just docs-ci`.
///
/// Reads `CARGO_MANIFEST_DIR` at runtime rather than via `env!`, which would
/// bake in the path of whichever worktree compiled the test binary.
#[test]
fn doc_flags_match_docs_ci() {
    let crate_dir = env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR: run tests via `cargo test` or `cargo nextest`");
    let justfile = fs::read_to_string(Utf8Path::new(&crate_dir).join("../../../justfile"))
        .expect("the workspace justfile to be readable");

    let recipe = justfile
        .split_once("\ndocs-ci:")
        .expect("the justfile to define `docs-ci`")
        .1;
    let recipe = recipe
        .split_once("\n\n#")
        .map_or(recipe, |(recipe, _)| recipe);

    let rustdocflags = recipe
        .lines()
        .find_map(|line| line.trim().strip_prefix("export RUSTDOCFLAGS="))
        .expect("`docs-ci` to export `RUSTDOCFLAGS`");
    assert_eq!(rustdocflags.trim_matches('"'), DOCS_RUSTDOCFLAGS);

    let cargo_doc = recipe
        .lines()
        .find(|line| line.trim().starts_with("cargo doc "))
        .expect("`docs-ci` to run `cargo doc`");
    let ci_flags: Vec<&str> = cargo_doc.split_whitespace().collect();
    // `doc_mergeable_info` sets `CARGO_UNSTABLE_RUSTDOC_MERGEABLE_INFO` rather
    // than passing `-Zrustdoc-mergeable-info` on the command line.
    for flag in DOC_ARGS[3..].iter().chain([&"-Zrustdoc-mergeable-info"]) {
        assert!(
            ci_flags.contains(flag),
            "`docs-ci` runs `{}` without `{flag}`",
            cargo_doc.trim()
        );
    }
}

/// The first command `runner` ran whose first argument is `arg`.
///
/// `cargo_check` runs cargo twice, so the clippy pass has to be picked out
/// rather than assumed to be last.
fn call_with_arg(runner: &MockProcessRunner, arg: &str) -> Option<ProcessSpec> {
    runner
        .calls()
        .into_iter()
        .find(|call| call.args.first().is_some_and(|first| first == arg))
}
