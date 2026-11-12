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

const NULL_OID: &str = "0000000000000000000000000000000000000000";

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

/// The answer to a command a test didn't script.
fn unexpected(spec: &ProcessSpec) -> io::Result<ProcessOutput> {
    Err(io::Error::other(format!("unexpected command: {spec}")))
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

    // `CX-003` is owned by `main` and deduplicated from `dry`; `iosp/violation`
    // is exempt; and `SRP-002` is a file-level rule, so its symbol is empty
    // even though line 1 spans `run`.
    assert_eq!(report.findings, vec![
        finding("src/lib.rs", 1, "SRP-002", "", "lib: 900 production lines"),
        finding("src/lib.rs", 2, "CX-003", "run", "Magic number 42 in run"),
        finding("src/lib.rs", 5, "DRY-002", "unused", "dead code: unused"),
    ]);
}

#[test]
fn report_for_a_rule_lists_only_its_findings() {
    let (_dir, workspace) = workspace();
    let runner = MockProcessRunner::responding(|spec| {
        if is_pass(spec, "dry.toml") {
            return ok(sarif(&[("DRY-002", "src/lib.rs", 5, "dead code: unused")]));
        }
        ok(sarif(&[
            ("CX-003", "src/lib.rs", 2, "Magic number 42 in run"),
            ("SRP-002", "src/lib.rs", 1, "lib: 900 production lines"),
        ]))
    });

    let report = workspace.report_rule(&runner, "CX-003").unwrap();

    assert_eq!(report.findings, vec![finding(
        "src/lib.rs",
        2,
        "CX-003",
        "run",
        "Magic number 42 in run"
    )]);
}

#[test]
fn report_for_an_exempt_rule_is_an_error() {
    let (_dir, workspace) = workspace();
    // Return a finding for the exempt rule so filtering instead of rejecting
    // would produce an empty `Ok` report rather than `Err`.
    let runner = MockProcessRunner::responding(|_| {
        ok(sarif(&[(
            "iosp/violation",
            "src/lib.rs",
            1,
            "IOSP violation in run",
        )]))
    });

    let error = workspace
        .report_rule(&runner, "iosp/violation")
        .unwrap_err();

    assert!(
        matches!(&error, Error::Exempt { rule, .. } if rule == "iosp/violation"),
        "{error:?}"
    );
    // Exempt rules are rejected from the policy without running `rustqual`.
    assert_eq!(runner.calls(), vec![]);
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
/// Resolves `git merge-base HEAD main` to `0123abcd` and only accepts that
/// commit for `git worktree add`.
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
            _ => unexpected(spec),
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
            _ => unexpected(spec),
        }
    });

    let error = workspace.diff(&runner, "main").unwrap_err();

    assert!(matches!(error, Error::Sarif { .. }), "{error:?}");
    assert_eq!(worktree_commands(&runner), vec![
        "worktree add",
        "worktree remove"
    ]);
}

const RUN: &str = indoc! {"
    pub fn run() -> u32 {
        42
    }
"};

/// Answers a `diff` run in which the magic number in `run` is in `src/lib.rs`
/// at the base and in `head_file` in the working tree.
///
/// `base_source` is what `head_file` held at the base commit, or `None` when
/// the commit has no such file.
fn moved_finding_responder(
    root: Utf8PathBuf,
    head_file: &'static str,
    base_source: Option<&'static str>,
) -> MockProcessRunner {
    MockProcessRunner::responding(move |spec| {
        let args: Vec<&str> = spec.args.iter().map(String::as_str).collect();
        match (spec.program.as_str(), args.as_slice()) {
            ("git", ["merge-base", "HEAD", "main"]) => ok("0123abcd\n"),
            ("git", ["worktree", "add", "--detach", tree, "0123abcd"]) => {
                write_lib(Utf8Path::new(tree));
                ok("")
            }
            ("git", ["worktree", "remove", ..] | ["diff", ..]) => ok(""),
            ("git", ["ls-tree", "-z", "--name-only", "0123abcd", "--", path])
                if *path == head_file =>
            {
                ok(base_source.map_or_else(String::new, |_| format!("{head_file}\0")))
            }
            ("git", ["show", object]) if *object == format!("0123abcd:{head_file}") => {
                ok(base_source.unwrap_or_default())
            }
            ("rustqual", ["--version"]) => ok("rustqual 1.8.2\n"),
            _ if is_pass(spec, "dry.toml") => ok(sarif(&[])),
            _ if is_pass(spec, "config.toml") => {
                let file = if spec.dir == root {
                    head_file
                } else {
                    "src/lib.rs"
                };
                ok(sarif(&[("CX-003", file, 2, "Magic number 42 in run")]))
            }
            _ => unexpected(spec),
        }
    })
}

fn ran_git(runner: &MockProcessRunner, subcommand: &str) -> bool {
    runner
        .calls()
        .iter()
        .any(|spec| spec.program == "git" && spec.args.first().is_some_and(|arg| arg == subcommand))
}

#[test]
fn diff_follows_a_function_that_moved_to_a_new_file() {
    let (dir, workspace) = workspace();
    fs::write(dir.path().join("src/lib.rs"), "fn unused() {}\n").unwrap();
    fs::write(dir.path().join("src/moved.rs"), RUN).unwrap();
    let runner = moved_finding_responder(dir.path().to_owned(), "src/moved.rs", None);

    let verdict = workspace.diff(&runner, "main").unwrap();

    assert!(verdict.passed());
    assert_eq!(verdict.new, vec![]);
    assert!(ran_git(&runner, "ls-tree"));
}

/// `src/lib.rs` still defines `run`, so its finding was fixed and not moved:
/// the `run` in `src/other.rs` is another function, and its finding is new.
#[test]
fn diff_reports_a_finding_in_a_namesake_when_the_function_stayed() {
    let (dir, workspace) = workspace();
    fs::write(dir.path().join("src/other.rs"), RUN).unwrap();
    let runner = moved_finding_responder(dir.path().to_owned(), "src/other.rs", Some(RUN));

    let verdict = workspace.diff(&runner, "main").unwrap();

    assert_eq!(verdict.new, vec![NewFinding {
        finding: finding("src/other.rs", 2, "CX-003", "run", "Magic number 42 in run"),
        shared: None,
    }]);
    // The working tree settled it; the base commit was not consulted.
    assert!(!ran_git(&runner, "ls-tree"));
}

/// `run` is gone from `src/lib.rs`, but `src/other.rs` had its own `run` at the
/// base, so nothing moved into it.
#[test]
fn diff_reports_a_finding_in_a_function_the_destination_already_had() {
    let (dir, workspace) = workspace();
    fs::write(dir.path().join("src/lib.rs"), "fn unused() {}\n").unwrap();
    fs::write(dir.path().join("src/other.rs"), RUN).unwrap();
    let runner = moved_finding_responder(dir.path().to_owned(), "src/other.rs", Some(RUN));

    let verdict = workspace.diff(&runner, "main").unwrap();

    assert_eq!(verdict.new, vec![NewFinding {
        finding: finding("src/other.rs", 2, "CX-003", "run", "Magic number 42 in run"),
        shared: None,
    }]);
    assert!(ran_git(&runner, "show"));
}

/// The destination existed at the base without a `run`, so the function moved
/// into a file that was already there.
#[test]
fn diff_follows_a_function_that_moved_to_an_existing_file() {
    let (dir, workspace) = workspace();
    fs::write(dir.path().join("src/lib.rs"), "fn unused() {}\n").unwrap();
    fs::write(dir.path().join("src/other.rs"), RUN).unwrap();
    let runner = moved_finding_responder(
        dir.path().to_owned(),
        "src/other.rs",
        Some("fn helper() {}\n"),
    );

    let verdict = workspace.diff(&runner, "main").unwrap();

    assert!(verdict.passed());
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
fn diff_against_the_null_object_id_analyses_only_the_working_tree() {
    let (dir, workspace) = workspace();
    let runner = diff_responder(dir.path().to_owned());

    let verdict = workspace.diff(&runner, NULL_OID).unwrap();

    // `diff_responder` reports an unenforced `DRY-002` finding in the working
    // tree that is absent from the base, so `verdict.passed()` holds only
    // because base comparison was skipped.
    assert!(verdict.passed());
    assert_eq!(verdict.base, None);
    assert_eq!(
        verdict.head,
        BTreeMap::from([("dry".to_owned(), 1), ("main".to_owned(), 1)])
    );

    let programs: Vec<_> = runner
        .calls()
        .into_iter()
        .map(|spec| spec.program)
        .collect();
    assert_eq!(programs, vec!["rustqual", "rustqual"]);
}

#[test]
fn diff_against_the_null_object_id_fails_on_an_enforced_rule() {
    let (dir, workspace) = workspace();
    let policy = indoc! {r#"
        enforced = ["DRY-002"]

        [[pass]]
        name = "dry"
        config = "dry.toml"
        rules = ["DRY-*"]

        [[pass]]
        name = "main"
        config = "config.toml"
        rules = ["*"]
    "#};
    fs::write(dir.path().join(CONFIG_DIR).join("qual.toml"), policy).unwrap();
    let runner = diff_responder(dir.path().to_owned());

    let verdict = workspace.diff(&runner, NULL_OID).unwrap();

    assert!(!verdict.passed());
    assert_eq!(verdict.new, vec![]);
    assert_eq!(verdict.enforced, vec![finding(
        "src/lib.rs",
        5,
        "DRY-002",
        "unused",
        "dead code: unused"
    )]);
}

#[test]
fn diff_resolves_an_abbreviated_run_of_zeros_as_a_revision() {
    let (_dir, workspace) = workspace();
    let runner = MockProcessRunner::builder()
        .expect("git")
        .args(&["merge-base", "HEAD", "0000000"])
        .returns_error("fatal: Not a valid object name 0000000");

    let error = workspace.diff(&runner, "0000000").unwrap_err();

    assert!(matches!(error, Error::Command { .. }), "{error:?}");
}

#[test]
fn recognises_the_null_object_id_at_full_length_only() {
    assert!(is_null_oid(NULL_OID));
    assert!(is_null_oid(
        "0000000000000000000000000000000000000000000000000000000000000000"
    ));

    assert!(!is_null_oid(""));
    assert!(!is_null_oid("0000000"));
    assert!(!is_null_oid("0000000000000000000000000000000000000001"));
    assert!(!is_null_oid("00000000000000000000000000000000000000000"));
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
