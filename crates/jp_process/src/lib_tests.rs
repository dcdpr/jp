use camino::Utf8Path;

use super::*;

#[test]
fn a_spec_reads_as_the_command_it_runs() {
    let spec = ProcessSpec::new("git", ["status", "--porcelain"], "/repo");

    assert_eq!(spec.to_string(), "git status --porcelain");
}

#[test]
fn an_exit_code_reads_as_its_number_or_as_a_signal() {
    assert_eq!(ExitCode::from_code(3).to_string(), "3");
    assert_eq!(ExitCode::from(None).to_string(), "terminated by signal");
}

#[test]
fn only_exit_code_zero_is_success() {
    assert!(ExitCode::success().is_success());
    assert!(!ExitCode::from_code(1).is_success());
    assert!(!ExitCode::from(None).is_success());
}

/// The shorthands describe the same run `execute` is given.
#[test]
fn the_shorthands_run_what_they_describe() {
    let runner = MockProcessRunner::responding(|_| {
        Ok(ProcessOutput {
            stdout: String::new(),
            stderr: String::new(),
            status: ExitCode::success(),
        })
    });
    let dir = Utf8Path::new("/repo");

    runner.run("git", &["status"], dir).unwrap();
    runner
        .run_with_env("git", &["log"], dir, &[("GIT_PAGER", "cat")])
        .unwrap();
    runner
        .run_with_env_and_stdin("git", &["apply"], dir, &[], Some("patch"))
        .unwrap();
    runner
        .run_with_opts("wc", &[], dir, &RunnerOpts {
            clean_env: true,
            ..RunnerOpts::default()
        })
        .unwrap();

    assert_eq!(runner.calls(), vec![
        ProcessSpec::new("git", ["status"], dir),
        ProcessSpec {
            env: vec![("GIT_PAGER".into(), "cat".into())],
            ..ProcessSpec::new("git", ["log"], dir)
        },
        ProcessSpec {
            stdin: Some("patch".into()),
            ..ProcessSpec::new("git", ["apply"], dir)
        },
        ProcessSpec {
            clean_env: true,
            ..ProcessSpec::new("wc", Vec::<String>::new(), dir)
        },
    ]);
}
