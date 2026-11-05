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
