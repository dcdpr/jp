use std::io;

use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::{Utf8TempDir, tempdir};
use indoc::indoc;
use jp_tool::{Action, Outcome};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
use crate::util::runner::{ExitCode, MockProcessRunner, ProcessOutput, RunnerOpts};

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

fn error_message(outcome: &Outcome) -> &str {
    match outcome {
        Outcome::Error { message, .. } => message,
        _ => panic!("Expected Outcome::Error, got: {outcome:?}"),
    }
}

fn name(name: &str) -> PackageSpec {
    PackageSpec::Name(name.to_owned())
}

fn pinned(name: &str, version: &str) -> PackageSpec {
    PackageSpec::Pinned {
        name: name.to_owned(),
        version: Some(version.to_owned()),
    }
}

fn success(stderr: &str) -> ProcessOutput {
    ProcessOutput {
        stdout: String::new(),
        stderr: stderr.to_owned(),
        status: ExitCode::success(),
    }
}

/// Runner that rewrites `Cargo.lock` when invoked, so the before/after snapshot
/// taken by `cargo_update_impl` observes a real change.
struct MutatingRunner {
    lockfile: Utf8PathBuf,
    after: String,
    stderr: String,
}

impl ProcessRunner for MutatingRunner {
    fn run_with_opts(
        &self,
        _program: &str,
        _args: &[&str],
        _working_dir: &Utf8Path,
        _opts: &RunnerOpts<'_>,
    ) -> Result<ProcessOutput, io::Error> {
        fs::write(&self.lockfile, &self.after)?;
        Ok(success(&self.stderr))
    }
}

#[test]
fn bare_string_and_object_forms_both_deserialize() {
    let packages: Vec<PackageSpec> = serde_json::from_value(json!([
        "serde",
        {"name": "tokio"},
        {"name": "clap", "version": "4.5.0"},
        {"name": "regex", "version": null},
    ]))
    .unwrap();

    let parsed: Vec<(&str, Option<&str>)> =
        packages.iter().map(|p| (p.name(), p.version())).collect();

    assert_eq!(parsed, vec![
        ("serde", None),
        ("tokio", None),
        ("clap", Some("4.5.0")),
        ("regex", None),
    ]);
}

#[test]
fn single_package_updates_only_that_package() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .args(&["update", "--color=never", "serde"])
        .returns(success("    Updating serde v1.0.200 -> v1.0.210"));

    let result = cargo_update_impl(&ctx, &[name("serde")], &runner).unwrap();
    assert_eq!(
        result.unwrap_content(),
        "Updating serde v1.0.200 -> v1.0.210"
    );
}

/// Cargo rejects a whole batch when any spec is unknown, so each package is
/// updated on its own to keep one bad name from discarding the others.
#[test]
fn each_package_gets_its_own_invocation() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .args(&["update", "--color=never", "serde"])
        .returns(success("    Updating serde v1.0.200 -> v1.0.210"))
        .expect("cargo")
        .args(&["update", "--color=never", "tokio"])
        .returns(success("    Updating tokio v1.40.0 -> v1.44.0"));

    let result = cargo_update_impl(&ctx, &[name("serde"), name("tokio")], &runner).unwrap();
    assert_eq!(result.unwrap_content(), indoc! {"
            2/2 packages updated.

            Updating serde v1.0.200 -> v1.0.210

            Updating tokio v1.40.0 -> v1.44.0"});
}

#[test]
fn pinned_package_is_updated_with_precise() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .args(&["update", "--color=never", "serde", "--precise", "1.0.210"])
        .returns(success("    Updating serde v1.0.200 -> v1.0.210"));

    let result = cargo_update_impl(&ctx, &[pinned("serde", "1.0.210")], &runner).unwrap();
    assert_eq!(
        result.unwrap_content(),
        "Updating serde v1.0.200 -> v1.0.210"
    );
}

#[test]
fn each_pinned_package_gets_its_own_version() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .args(&["update", "--color=never", "serde", "--precise", "1.0.210"])
        .returns(success("    Updating serde v1.0.200 -> v1.0.210"))
        .expect("cargo")
        .args(&["update", "--color=never", "tokio", "--precise", "1.44.0"])
        .returns(success("    Updating tokio v1.40.0 -> v1.44.0"));

    let packages = [pinned("serde", "1.0.210"), pinned("tokio", "1.44.0")];
    let result = cargo_update_impl(&ctx, &packages, &runner).unwrap();
    assert_eq!(result.unwrap_content(), indoc! {"
            2/2 packages updated.

            Updating serde v1.0.200 -> v1.0.210

            Updating tokio v1.40.0 -> v1.44.0"});
}

#[test]
fn packages_are_updated_in_request_order() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .args(&["update", "--color=never", "serde"])
        .returns(success(""))
        .expect("cargo")
        .args(&["update", "--color=never", "tokio", "--precise", "1.44.0"])
        .returns(success(""))
        .expect("cargo")
        .args(&["update", "--color=never", "regex"])
        .returns(success(""));

    let packages = [name("serde"), pinned("tokio", "1.44.0"), name("regex")];
    let result = cargo_update_impl(&ctx, &packages, &runner).unwrap();
    assert_eq!(result.unwrap_content(), "3/3 packages updated.");
}

#[test]
fn lockfile_changes_are_reported_as_a_diff() {
    let (_dir, ctx) = ctx();
    let lockfile = ctx.root.join("Cargo.lock");
    fs::write(&lockfile, indoc! {r#"
            [[package]]
            name = "serde"
            version = "1.0.200"

            [[package]]
            name = "serde_core"
            version = "1.0.200"
        "#})
    .unwrap();

    // The transitive `serde_core` bump is the reason the diff is worth
    // reporting: cargo was only asked to update `serde`.
    let runner = MutatingRunner {
        lockfile,
        after: indoc! {r#"
            [[package]]
            name = "serde"
            version = "1.0.210"

            [[package]]
            name = "serde_core"
            version = "1.0.210"
        "#}
        .to_owned(),
        stderr: "    Updating serde v1.0.200 -> v1.0.210".to_owned(),
    };

    let result = cargo_update_impl(&ctx, &[name("serde")], &runner).unwrap();
    assert_eq!(result.unwrap_content(), indoc! {r#"
            ```diff
            Updating serde v1.0.200 -> v1.0.210

            --- Cargo.lock
            +++ Cargo.lock
            @@ -1,7 +1,7 @@
             [[package]]
             name = "serde"
            -version = "1.0.200"
            +version = "1.0.210"
             
             [[package]]
             name = "serde_core"
            -version = "1.0.200"
            +version = "1.0.210"
            ```"#});
}

#[test]
fn an_unchanged_lockfile_produces_no_diff() {
    let (_dir, ctx) = ctx();
    fs::write(ctx.root.join("Cargo.lock"), "version = 4\n").unwrap();

    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns(success("    Updating crates.io index"));

    let result = cargo_update_impl(&ctx, &[name("serde")], &runner).unwrap();
    assert_eq!(result.unwrap_content(), "Updating crates.io index");
}

#[test]
fn nothing_reported_and_nothing_changed_says_so() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns(success(""));

    let result = cargo_update_impl(&ctx, &[name("serde")], &runner).unwrap();
    assert_eq!(
        result.unwrap_content(),
        "Nothing to update, the lockfile is unchanged."
    );
}

#[test]
fn a_failing_package_does_not_stop_the_others() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .args(&["update", "--color=never", "srde"])
        .returns(ProcessOutput {
            stdout: String::new(),
            stderr: "error: package ID specification `srde` did not match any packages".to_owned(),
            status: ExitCode::from_code(101),
        })
        .expect("cargo")
        .args(&["update", "--color=never", "tokio"])
        .returns(success("    Updating tokio v1.40.0 -> v1.44.0"));

    let result = cargo_update_impl(&ctx, &[name("srde"), name("tokio")], &runner).unwrap();
    assert_eq!(result.unwrap_content(), indoc! {"
            1/2 packages updated.

            Updating tokio v1.40.0 -> v1.44.0

            Failed:
            * srde: error: package ID specification `srde` did not match any packages"});
}

#[test]
fn a_failed_pinned_package_is_reported_with_its_version() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .args(&["update", "--color=never", "serde"])
        .returns(success("    Updating serde v1.0.200 -> v1.0.210"))
        .expect("cargo")
        .args(&["update", "--color=never", "tokio", "--precise", "99.0.0"])
        .returns(ProcessOutput {
            stdout: String::new(),
            stderr: "error: no matching package named `tokio` found\nlocation searched: registry"
                .to_owned(),
            status: ExitCode::from_code(101),
        });

    let packages = [name("serde"), pinned("tokio", "99.0.0")];
    let result = cargo_update_impl(&ctx, &packages, &runner).unwrap();
    assert_eq!(result.unwrap_content(), indoc! {"
            1/2 packages updated.

            Updating serde v1.0.200 -> v1.0.210

            Failed:
            * tokio@99.0.0: error: no matching package named `tokio` found
              location searched: registry"});
}

#[test]
fn every_package_failing_is_an_error() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns(ProcessOutput {
            stdout: String::new(),
            stderr: "error: package ID specification `srde` did not match any packages".to_owned(),
            status: ExitCode::from_code(101),
        })
        .expect("cargo")
        .returns(ProcessOutput {
            stdout: String::new(),
            stderr: "error: package ID specification `tokoi` did not match any packages".to_owned(),
            status: ExitCode::from_code(101),
        });

    let result = cargo_update_impl(&ctx, &[name("srde"), name("tokoi")], &runner).unwrap();
    assert_eq!(error_message(&result), indoc! {"
            cargo update failed:
            * srde: error: package ID specification `srde` did not match any packages
            * tokoi: error: package ID specification `tokoi` did not match any packages"});
}

#[test]
fn no_packages_is_rejected_without_running_cargo() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::never_called();

    let result = cargo_update_impl(&ctx, &[], &runner).unwrap();
    assert_eq!(error_message(&result), "At least one package is required.");
}

#[test]
fn package_name_looking_like_a_flag_is_rejected_without_running_cargo() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::never_called();

    let result = cargo_update_impl(&ctx, &[name("--workspace")], &runner).unwrap();
    assert_eq!(
        error_message(&result),
        "Invalid package name '--workspace': must not start with '-'."
    );
}

#[test]
fn blank_package_name_is_rejected_without_running_cargo() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::never_called();

    let result = cargo_update_impl(&ctx, &[name("  ")], &runner).unwrap();
    assert_eq!(error_message(&result), "Empty package name.");
}

#[test]
fn version_looking_like_a_flag_is_rejected_without_running_cargo() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::never_called();

    let result = cargo_update_impl(&ctx, &[pinned("serde", "-Zfoo")], &runner).unwrap();
    assert_eq!(
        error_message(&result),
        "Invalid version '-Zfoo': must not start with '-'."
    );
}

/// One bad package rejects the whole call: the guard runs before any cargo
/// invocation, so nothing is left half-applied.
#[test]
fn an_invalid_package_rejects_the_whole_call() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::never_called();

    let result = cargo_update_impl(&ctx, &[name("serde"), name("-x")], &runner).unwrap();
    assert_eq!(
        error_message(&result),
        "Invalid package name '-x': must not start with '-'."
    );
}

#[test]
fn lockfile_diff_returns_none_when_unchanged() {
    assert_eq!(
        lockfile_diff(Some("version = 4\n"), Some("version = 4\n")),
        None
    );
}

#[test]
fn lockfile_diff_returns_none_when_the_lockfile_is_missing() {
    assert_eq!(lockfile_diff(None, Some("version = 4\n")), None);
    assert_eq!(lockfile_diff(Some("version = 4\n"), None), None);
}
