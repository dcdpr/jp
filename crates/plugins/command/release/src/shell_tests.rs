use super::*;
use crate::repository_testing::Fixture;

#[test]
fn git_returns_what_it_printed() {
    let fixture = Fixture::new();

    let branch = fixture
        .shell()
        .git(fixture.root(), &["branch", "--show-current"])
        .unwrap();

    assert_eq!(branch, "main\n");
}

#[test]
fn a_failing_git_command_is_named_with_what_git_said() {
    let fixture = Fixture::new();

    let error = fixture
        .shell()
        .git(fixture.root(), &[
            "-c",
            "core.quotePath=false",
            "show",
            "no-such-commit",
        ])
        .unwrap_err();

    assert!(error.starts_with("`git show` failed: fatal: "), "{error}");
}

/// The environment a test pins reaches every program the shell runs.
#[test]
fn every_program_runs_with_the_shells_environment() {
    let fixture = Fixture::new();

    let ran = fixture
        .shell()
        .run(fixture.root(), "git", &["var", "GIT_AUTHOR_IDENT"])
        .unwrap();

    assert!(ran.success);
    assert!(
        ran.stdout.starts_with("Tester <tester@example.com> "),
        "{}",
        ran.stdout
    );
}

#[test]
fn a_program_explains_itself_on_stderr_or_else_on_stdout() {
    let ran = |stdout: &str, stderr: &str| Ran {
        success: false,
        stdout: stdout.to_owned(),
        stderr: stderr.to_owned(),
    };

    assert_eq!(ran("out\n", "err\n").said(), "err");
    assert_eq!(ran("out\n", " \n").said(), "out");
}

#[test]
fn a_streamed_program_hands_on_what_it_prints_on_either_stream() {
    let fixture = Fixture::new();
    let shell = fixture.shell();
    let mut lines = vec![];

    let printed = shell
        .stream(fixture.root(), "git", &["--version"], &mut |line| {
            lines.push(line.to_owned());
            Ok(())
        })
        .unwrap();
    let failed = shell
        .stream(
            fixture.root(),
            "git",
            &["rev-parse", "--verify", "nope"],
            &mut |line| {
                lines.push(line.to_owned());
                Ok(())
            },
        )
        .unwrap();

    assert!(printed.success());
    assert!(!failed.success());
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(lines[0].starts_with("git version "), "{lines:?}");
    assert_eq!(lines[1], "fatal: Needed a single revision");
}

/// More than a pipe holds, so a program whose output nobody read would never
/// end.
#[test]
fn a_streamed_program_runs_to_its_end_after_the_callback_fails() {
    let fixture = Fixture::new();
    fixture.write("big.txt", &"a line of text\n".repeat(20_000));
    let blob = fixture
        .git(&["hash-object", "-w", "big.txt"])
        .trim()
        .to_owned();
    let mut seen = 0;

    let error = fixture
        .shell()
        .stream(
            fixture.root(),
            "git",
            &["cat-file", "-p", &blob],
            &mut |_| {
                seen += 1;
                Err("the host is gone".to_owned())
            },
        )
        .unwrap_err();

    assert_eq!(error, "the host is gone");
    assert_eq!(seen, 1);
}

#[test]
fn a_program_that_is_not_there_is_named() {
    let fixture = Fixture::new();

    let error = fixture
        .shell()
        .run(fixture.root(), "jp-release-no-such-program", &["x"])
        .unwrap_err();

    assert!(
        error.starts_with("`jp-release-no-such-program` couldn't be run: "),
        "{error}"
    );
}
