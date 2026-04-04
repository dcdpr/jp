use std::path::Path;

use assert_matches::assert_matches;
use camino::Utf8Path;
use camino_tempfile::tempdir;
use jp_tool::{Action, Outcome};

use super::*;
use crate::util::runner::MockProcessRunner;
#[cfg(unix)]
use crate::util::runner::{ExitCode, ProcessOutput};

fn ctx() -> (camino_tempfile::Utf8TempDir, Context) {
    let dir = tempdir().unwrap();
    let ctx = Context {
        root: dir.path().to_owned(),
        action: Action::Run,
    };
    (dir, ctx)
}

/// Simulated filesystem for tests. Only these paths "exist".
fn test_exists(p: &Path) -> bool {
    let s = p.to_string_lossy();
    [
        "/etc/passwd",
        "/etc/shadow",
        "/etc/hosts",
        "/tmp/secret.txt",
        "/usr/bin/date",
        "/home/user/.ssh/id_rsa",
    ]
    .iter()
    .any(|known| s == *known)
}

#[test]
fn rejects_unknown_util() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::never_called();

    let result = unix_utils_impl(&ctx, "rm", None, None, &runner);
    assert_matches!(result.unwrap(), Outcome::Error { message, .. } => {
        assert!(message.contains("Unknown util"), "got: {message}");
    });
}

#[cfg(unix)]
#[test]
fn runs_allowed_util_with_args() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::builder()
        .expect_any()
        .returns_success("2025-01-15\n");

    let args = Some(OneOrMany::One("+%Y-%m-%d".into()));
    let result = unix_utils_impl(&ctx, "date", args, None, &runner);
    let content = result.unwrap().unwrap_content();

    assert!(content.contains("2025-01-15"), "got: {content}");
}

#[cfg(unix)]
#[test]
fn runs_util_without_args() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::builder()
        .expect_any()
        .returns_success("550e8400-e29b-41d4-a716-446655440000\n");

    let result = unix_utils_impl(&ctx, "uuidgen", None, None, &runner);
    let content = result.unwrap().unwrap_content();

    assert!(
        content.contains("550e8400-e29b-41d4-a716-446655440000"),
        "got: {content}"
    );
}

#[cfg(unix)]
#[test]
fn includes_stderr_on_failure() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::builder()
        .expect_any()
        .returns(ProcessOutput {
            stdout: String::new(),
            stderr: "parse error\n".into(),
            status: ExitCode::from_code(1),
        });

    let result = unix_utils_impl(&ctx, "bc", Some(OneOrMany::Many(vec![])), None, &runner);
    let content = result.unwrap().unwrap_content();

    assert!(content.contains("parse error"), "got: {content}");
    assert!(content.contains("status"), "got: {content}");
}

#[cfg(unix)]
#[test]
fn omits_empty_fields() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::builder()
        .expect_any()
        .returns_success("Mon Jan 15\n");

    let result = unix_utils_impl(&ctx, "date", None, None, &runner);
    let content = result.unwrap().unwrap_content();

    assert!(content.contains("Mon Jan 15"), "got: {content}");
    assert!(!content.contains("stderr"), "got: {content}");
    assert!(!content.contains("error"), "got: {content}");
    assert!(!content.contains("status"), "got: {content}");
}

#[cfg(unix)]
#[test]
fn truncates_large_stdout() {
    let (_dir, ctx) = ctx();
    let big_output = "x".repeat(MAX_OUTPUT_BYTES + 500);
    let runner = MockProcessRunner::builder()
        .expect_any()
        .returns_success(&big_output);

    let result = unix_utils_impl(&ctx, "base64", None, Some("data"), &runner);
    let content = result.unwrap().unwrap_content();

    assert!(content.contains("[Truncated:"), "got: {content}");
    assert!(content.len() < big_output.len(), "output should be smaller");
}

#[cfg(unix)]
#[test]
fn does_not_truncate_small_output() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::builder()
        .expect_any()
        .returns_success("  42 Cargo.toml\n");

    let result = unix_utils_impl(&ctx, "wc", None, None, &runner);
    let content = result.unwrap().unwrap_content();

    assert!(!content.contains("[Truncated:"), "got: {content}");
}

#[test]
fn validate_args_cases() {
    let root = Utf8Path::new("/workspace");

    let cases: &[(&str, &[&str], bool)] = &[
        // Safe relative paths
        ("bare filename", &["Cargo.lock"], true),
        ("relative file", &["src/main.rs"], true),
        ("dotdot stays inside", &["src/../Cargo.toml"], true),
        ("simple number", &["256"], true),
        ("flag", &["-l"], true),
        ("long flag", &["--decode"], true),
        ("format string", &["+%Y-%m-%d"], true),
        // jq filters resolve harmlessly inside workspace
        ("jq filter", &[".foo.bar"], true),
        ("jq recursive", &[".[].name"], true),
        // Non-existent absolute paths are harmless
        ("nonexistent absolute", &["/nonexistent/path"], true),
        ("nonexistent in flag", &["--x=/nonexistent"], true),
        // Tilde — always rejected
        ("tilde home", &["~/secret.json"], false),
        ("bare tilde", &["~"], false),
        ("tilde in flag value", &["-f~/secret"], false),
        ("tilde after equals", &["--config=~/foo"], false),
        // Dotdot escapes — rejected regardless of existence
        ("dotdot escape", &["../../etc/passwd"], false),
        ("sneaky escape", &["src/../../../etc/hosts"], false),
        ("bare dotdot", &[".."], false),
        ("dot-slash escape", &["./../../etc/passwd"], false),
        // Absolute paths to "existing" system paths
        ("absolute /etc/passwd", &["/etc/passwd"], false),
        ("absolute /etc/shadow", &["/etc/shadow"], false),
        ("embedded in short flag", &["-f/etc/passwd"], false),
        ("flag equals absolute", &["--file=/etc/passwd"], false),
        (
            "files0-from attack",
            &["--files0-from=/tmp/secret.txt"],
            false,
        ),
        // Delimiter-separated paths (caught by pass 2)
        ("colon separated", &["/etc/passwd:/etc/shadow"], false),
        ("semicolon separated", &["/etc/hosts;/etc/shadow"], false),
        ("comma separated", &["/etc/passwd,/etc/shadow"], false),
        // Fragment-level normalization (pass 2 normalizes after splitting)
        ("dotdot in fragment", &["x:bar/../../../etc/passwd"], false),
        // Mixed: one bad arg fails the set
        ("safe then escape", &["src/lib.rs", "../../out"], false),
    ];

    for &(label, args, expect_ok) in cases {
        let args: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
        let result = validate_args(root, &args, test_exists);
        assert_eq!(result.is_ok(), expect_ok, "{label}: {result:?}");
    }
}

#[test]
fn validate_args_scans_all_positions() {
    let root = Utf8Path::new("/workspace");

    // Paths buried after arbitrary prefix characters
    assert!(validate_args(root, &["-f/etc/passwd".into()], test_exists).is_err());
    assert!(validate_args(root, &["xyzzy/etc/passwd".into()], test_exists).is_err());
    assert!(validate_args(root, &["abc~/secret".into()], test_exists).is_err());
}

#[cfg(unix)]
#[test]
fn absolute_path_blocks_execution() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::never_called();

    let args = Some(OneOrMany::One("/etc/passwd".into()));
    let result = unix_utils_impl(&ctx, "wc", args, None, &runner);

    assert_matches!(result.unwrap(), Outcome::Error { .. });
}

#[cfg(unix)]
#[test]
fn flag_value_escape_blocks_execution() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::never_called();

    let args = Some(OneOrMany::One("--files0-from=/etc/passwd".into()));
    let result = unix_utils_impl(&ctx, "wc", args, None, &runner);

    assert_matches!(result.unwrap(), Outcome::Error { .. });
}

#[cfg(unix)]
#[test]
fn tilde_blocks_execution() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::never_called();

    let args = Some(OneOrMany::One("~/.ssh/id_rsa".into()));
    let result = unix_utils_impl(&ctx, "file", args, None, &runner);

    assert_matches!(result.unwrap(), Outcome::Error { message, .. } => {
        assert!(message.contains("Home directory"), "got: {message}");
    });
}

#[cfg(unix)]
#[test]
fn safe_path_reaches_runner() {
    let (_dir, ctx) = ctx();
    let runner = MockProcessRunner::builder()
        .expect_any()
        .returns_success("  42 src/main.rs\n");

    let args = Some(OneOrMany::Many(vec!["-l".into(), "src/main.rs".into()]));
    let result = unix_utils_impl(&ctx, "wc", args, None, &runner);

    assert!(result.unwrap().unwrap_content().contains("42"));
}
