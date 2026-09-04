use camino::Utf8PathBuf;
use pretty_assertions::assert_eq;

use super::*;

fn spec() -> RunSpec {
    RunSpec {
        image: "ubuntu".to_owned(),
        mounts: vec![],
        envs: vec![],
        workdir: None,
        script: "set -euo pipefail\necho hi\n".to_owned(),
    }
}

#[test]
fn a_bare_run_removes_the_container() {
    let (program, args) = argv(Runtime::Docker, &spec());

    assert_eq!(program, "docker");
    assert_eq!(args, vec![
        "run",
        "--rm",
        "ubuntu",
        "bash",
        "-c",
        "set -euo pipefail\necho hi\n",
    ]);
}

/// Every supported runtime takes the same flags; only the program differs.
#[test]
fn each_runtime_keeps_the_same_arguments() {
    let expected = argv(Runtime::Docker, &spec()).1;

    for (runtime, program) in [(Runtime::Apple, "container"), (Runtime::Podman, "podman")] {
        let (actual_program, args) = argv(runtime, &spec());
        assert_eq!(actual_program, program);
        assert_eq!(args, expected);
    }
}

#[test]
fn mounts_are_read_only_and_set_the_working_directory() {
    let spec = RunSpec {
        mounts: vec![
            (
                Utf8PathBuf::from("/ws/crates"),
                "/workspace/crates".to_owned(),
            ),
            (
                Utf8PathBuf::from("/ws/src/foo.rs"),
                "/workspace/src/foo.rs".to_owned(),
            ),
        ],
        workdir: Some("/workspace".to_owned()),
        ..spec()
    };

    let (_, args) = argv(Runtime::Apple, &spec);

    assert_eq!(args, vec![
        "run",
        "--rm",
        "--volume",
        "/ws/crates:/workspace/crates:ro",
        "--volume",
        "/ws/src/foo.rs:/workspace/src/foo.rs:ro",
        "--workdir",
        "/workspace",
        "ubuntu",
        "bash",
        "-c",
        "set -euo pipefail\necho hi\n",
    ]);
}

/// Variables are named, never valued, on the command line — a value in `argv`
/// is readable by any process on the host through `ps`.
#[test]
fn environment_variables_are_forwarded_by_name_only() {
    let spec = RunSpec {
        envs: vec!["GITHUB_TOKEN".to_owned(), "CI".to_owned()],
        ..spec()
    };

    let (_, args) = argv(Runtime::Podman, &spec);

    assert_eq!(args, vec![
        "run",
        "--rm",
        "--env",
        "GITHUB_TOKEN",
        "--env",
        "CI",
        "ubuntu",
        "bash",
        "-c",
        "set -euo pipefail\necho hi\n",
    ]);
}

#[test]
fn the_configured_image_is_used() {
    let spec = RunSpec {
        image: "ghcr.io/example/jp-bash:v1".to_owned(),
        ..spec()
    };

    let (_, args) = argv(Runtime::Docker, &spec);

    assert_eq!(args[2], "ghcr.io/example/jp-bash:v1");
}
