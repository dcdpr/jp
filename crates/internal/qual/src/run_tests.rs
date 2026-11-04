use std::io;

use camino_tempfile::Utf8TempDir;
use indoc::indoc;
use jp_process::{MockProcessRunner, ProcessOutput, ProcessSpec};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
use crate::NewFinding;

const POLICY: &str = indoc! {r#"
    exempt = ["iosp/violation"]

    [[pass]]
    name = "dry"
    config = "dry.toml"
    rules = ["DRY-*"]

    [[pass]]
    name = "main"
    config = "config.toml"
    rules = ["*"]
"#};

// Line numbers in the reports below refer to this source: `run` spans lines 1
// to 3, `unused` is line 5.
const LIB: &str = indoc! {"
    pub fn run() -> u32 {
        42
    }

    fn unused() {}
"};

/// A checkout holding the policy, two empty rustqual configs, and `LIB`.
fn workspace() -> (Utf8TempDir, Workspace) {
    let dir = camino_tempfile::tempdir().unwrap();
    let config = dir.path().join(CONFIG_DIR);
    fs::create_dir_all(&config).unwrap();
    fs::write(config.join("qual.toml"), POLICY).unwrap();
    fs::write(config.join("config.toml"), "").unwrap();
    fs::write(config.join("dry.toml"), "").unwrap();
    write_lib(dir.path());

    let workspace = Workspace::new(dir.path());
    (dir, workspace)
}

fn write_lib(tree: &Utf8Path) {
    fs::create_dir_all(tree.join("src")).unwrap();
    fs::write(tree.join("src/lib.rs"), LIB).unwrap();
}

/// A rustqual SARIF report holding `(rule, file, line, message)` results.
fn sarif(results: &[(&str, &str, usize, &str)]) -> String {
    let results: Vec<_> = results
        .iter()
        .map(|(rule, file, line, message)| {
            json!({
                "ruleId": rule,
                "level": "warning",
                "message": { "text": message },
                "locations": [{
                    "physicalLocation": {
                        "artifactLocation": { "uri": file },
                        "region": { "startLine": line }
                    }
                }]
            })
        })
        .collect();

    json!({
        "version": "2.1.0",
        "runs": [{
            "tool": { "driver": { "name": "rustqual", "rules": [] } },
            "results": results
        }]
    })
    .to_string()
}

/// A successful run printing `stdout`, as a responder answers.
#[expect(
    clippy::unnecessary_wraps,
    reason = "responders answer with `io::Result`"
)]
fn ok(stdout: impl Into<String>) -> io::Result<ProcessOutput> {
    Ok(ProcessOutput {
        stdout: stdout.into(),
        stderr: String::new(),
        status: ExitCode::success(),
    })
}

fn is_pass(spec: &ProcessSpec, config: &str) -> bool {
    spec.program == "rustqual" && spec.args.get(1).is_some_and(|path| path.ends_with(config))
}

fn finding(file: &str, line: usize, rule: &str, symbol: &str, message: &str) -> Finding {
    Finding {
        file: file.to_owned(),
        line,
        rule: rule.to_owned(),
        symbol: symbol.to_owned(),
        message: message.to_owned(),
        pass: if rule.starts_with("DRY-") {
            "dry"
        } else {
            "main"
        }
        .to_owned(),
    }
}

#[test]
fn report_reads_each_rule_from_its_pass_and_names_its_symbol() {
    let (_dir, workspace) = workspace();
    let runner = MockProcessRunner::responding(|spec| {
        if is_pass(spec, "dry.toml") {
            return ok(sarif(&[
                ("DRY-002", "src/lib.rs", 5, "dead code: unused"),
                ("CX-003", "src/lib.rs", 2, "Magic number 42 in run"),
            ]));
        }
        ok(sarif(&[
            ("CX-003", "src/lib.rs", 2, "Magic number 42 in run"),
            ("iosp/violation", "src/lib.rs", 1, "IOSP violation in run"),
            ("SRP-002", "src/lib.rs", 1, "lib: 900 production lines"),
        ]))
    });

    let report = workspace.report(&runner).unwrap();

    // The DRY pass's magic number is the main pass's, read once; IOSP is
    // exempt; and a file-length finding names no item, though line 1 is in
    // `run`.
    assert_eq!(report.findings, vec![
        finding("src/lib.rs", 1, "SRP-002", "", "lib: 900 production lines"),
        finding("src/lib.rs", 2, "CX-003", "run", "Magic number 42 in run"),
        finding("src/lib.rs", 5, "DRY-002", "unused", "dead code: unused"),
    ]);
}

#[test]
fn a_failing_rustqual_run_is_an_error() {
    let (_dir, workspace) = workspace();
    let runner = MockProcessRunner::responding(|_| {
        Ok(ProcessOutput {
            stdout: String::new(),
            stderr: "Error: unknown field `foo`".to_owned(),
            status: ExitCode::from_code(2),
        })
    });

    let error = workspace.report(&runner).unwrap_err();

    assert!(
        matches!(&error, Error::Command { status, stderr, .. }
            if *status == ExitCode::from_code(2) && stderr == "Error: unknown field `foo`"),
        "{error:?}"
    );
}

#[test]
fn a_missing_rustqual_is_an_error() {
    let (_dir, workspace) = workspace();
    let runner = MockProcessRunner::builder()
        .expect("rustqual")
        .fails_to_spawn();

    let error = workspace.report(&runner).unwrap_err();

    assert!(matches!(error, Error::Spawn { .. }), "{error:?}");
}

#[test]
fn a_report_that_cannot_be_read_is_an_error() {
    let (_dir, workspace) = workspace();
    let runner = MockProcessRunner::responding(|_| ok(""));

    let error = workspace.report(&runner).unwrap_err();

    assert!(
        matches!(&error, Error::Sarif { pass, .. } if pass == "dry"),
        "{error:?}"
    );
}

#[test]
fn a_file_rustqual_skipped_is_an_error() {
    let (_dir, workspace) = workspace();
    let runner = MockProcessRunner::responding(|_| {
        Ok(ProcessOutput {
            stdout: sarif(&[]),
            stderr: "Warning: Could not parse src/x.rs: expected `;`\n".to_owned(),
            status: ExitCode::success(),
        })
    });

    let error = workspace.report(&runner).unwrap_err();

    assert_eq!(
        error.to_string(),
        "rustqual's dry pass skipped files:\nWarning: Could not parse src/x.rs: expected `;`"
    );
}

#[test]
fn a_finding_in_a_file_that_does_not_parse_is_an_error() {
    let (dir, workspace) = workspace();
    fs::write(dir.path().join("src/bad.rs"), "fn (").unwrap();
    let runner = MockProcessRunner::responding(|spec| {
        if is_pass(spec, "dry.toml") {
            return ok(sarif(&[("DRY-002", "src/bad.rs", 1, "dead code: f")]));
        }
        ok(sarif(&[]))
    });

    let error = workspace.report(&runner).unwrap_err();

    assert!(matches!(error, Error::Parse { .. }), "{error:?}");
}

#[test]
fn an_invalid_policy_is_an_error() {
    let (dir, workspace) = workspace();
    fs::write(dir.path().join(CONFIG_DIR).join("qual.toml"), "pass = []").unwrap();

    let error = workspace
        .report(&MockProcessRunner::never_called())
        .unwrap_err();

    assert!(
        matches!(error, Error::Policy {
            source: PolicyError::NoPass,
            ..
        }),
        "{error:?}"
    );
}

/// Answers a full `diff` run: the base has a magic number in `run`, and the
/// working tree adds dead code in `unused`.
///
/// `main` forked from the working tree at `0123abcd`; only that commit can be
/// checked out as the base.
fn diff_responder(root: Utf8PathBuf) -> MockProcessRunner {
    MockProcessRunner::responding(move |spec| {
        let args: Vec<&str> = spec.args.iter().map(String::as_str).collect();
        match (spec.program.as_str(), args.as_slice()) {
            ("git", ["merge-base", "HEAD", "main"]) => ok("0123abcd\n"),
            ("git", ["worktree", "add", "--detach", tree, "0123abcd"]) => {
                write_lib(Utf8Path::new(tree));
                ok("")
            }
            ("git", ["worktree", "remove", ..] | ["diff", ..]) => ok(""),
            ("rustqual", ["--version"]) => ok("rustqual 1.8.2\n"),
            _ if is_pass(spec, "config.toml") => ok(sarif(&[(
                "CX-003",
                "src/lib.rs",
                2,
                "Magic number 42 in run",
            )])),
            _ if is_pass(spec, "dry.toml") && spec.dir == root => {
                ok(sarif(&[("DRY-002", "src/lib.rs", 5, "dead code: unused")]))
            }
            _ if is_pass(spec, "dry.toml") => ok(sarif(&[])),
            _ => Err(io::Error::other(format!("unexpected command: {spec}"))),
        }
    })
}

fn worktree_commands(runner: &MockProcessRunner) -> Vec<String> {
    runner
        .calls()
        .iter()
        .filter(|spec| spec.args.first().is_some_and(|arg| arg == "worktree"))
        .map(|spec| spec.args[..2].join(" "))
        .collect()
}

#[test]
fn diff_fails_on_a_finding_the_base_lacks() {
    let (dir, workspace) = workspace();
    let runner = diff_responder(dir.path().to_owned());

    let verdict = workspace.diff(&runner, "main").unwrap();

    assert!(!verdict.passed());
    assert_eq!(verdict.new, vec![NewFinding {
        finding: finding("src/lib.rs", 5, "DRY-002", "unused", "dead code: unused"),
        shared: None,
    }]);
    assert_eq!(worktree_commands(&runner), vec![
        "worktree add",
        "worktree remove"
    ]);
}

#[test]
fn diff_reads_a_cached_base_report_on_the_next_run() {
    let (dir, workspace) = workspace();
    let runner = diff_responder(dir.path().to_owned());

    let first = workspace.diff(&runner, "main").unwrap();
    let second = workspace.diff(&runner, "main").unwrap();

    assert_eq!(first, second);
    assert_eq!(worktree_commands(&runner), vec![
        "worktree add",
        "worktree remove"
    ]);
}

#[test]
fn diff_removes_the_base_worktree_when_its_analysis_fails() {
    let (dir, workspace) = workspace();
    let root = dir.path().to_owned();
    let runner = MockProcessRunner::responding(move |spec| {
        let args: Vec<&str> = spec.args.iter().map(String::as_str).collect();
        match (spec.program.as_str(), args.as_slice()) {
            ("git", ["merge-base", ..]) => ok("0123abcd\n"),
            ("git", ["worktree", "add", "--detach", tree, _]) => {
                write_lib(Utf8Path::new(tree));
                ok("")
            }
            ("git", ["worktree", "remove", ..]) => ok(""),
            ("rustqual", ["--version"]) => ok("rustqual 1.8.2\n"),
            ("rustqual", _) if spec.dir != root => ok("not sarif"),
            _ => Err(io::Error::other(format!("unexpected command: {spec}"))),
        }
    });

    let error = workspace.diff(&runner, "main").unwrap_err();

    assert!(matches!(error, Error::Sarif { .. }), "{error:?}");
    assert_eq!(worktree_commands(&runner), vec![
        "worktree add",
        "worktree remove"
    ]);
}

#[test]
fn diff_rejects_a_base_that_is_not_a_commit() {
    let (_dir, workspace) = workspace();
    let runner = MockProcessRunner::builder()
        .expect("git")
        .args(&["merge-base", "HEAD", "nope"])
        .returns_error("fatal: Not a valid object name nope");

    let error = workspace.diff(&runner, "nope").unwrap_err();

    assert!(matches!(error, Error::Command { .. }), "{error:?}");
}

#[test]
fn reads_guidance_for_rules_that_have_it() {
    let (dir, workspace) = workspace();
    let guidance_dir = dir.path().join(CONFIG_DIR).join(GUIDANCE_DIR);
    fs::create_dir_all(&guidance_dir).unwrap();
    fs::write(guidance_dir.join("CX-004.md"), "Return early.\n").unwrap();

    let guidance = workspace.guidance(["CX-004", "CX-001", "CX-004"]).unwrap();

    assert_eq!(guidance.get("CX-004"), Some("Return early."));
    assert_eq!(guidance.get("CX-001"), None);
}
