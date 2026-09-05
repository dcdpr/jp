use camino::Utf8PathBuf;
use pretty_assertions::assert_eq;

use super::*;
use crate::util::runner::MockProcessRunner;

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

fn install(script: &str) -> Install {
    Install {
        script: script.to_owned(),
        run_as: "nonroot".to_owned(),
    }
}

/// The tag is content-addressed, so a cache hit is the image this exact base
/// and script produced.
#[test]
fn the_tag_changes_with_every_input() {
    let base = image_tag("alpine", &install("apk add python3"));

    assert_eq!(base, image_tag("alpine", &install("apk add python3")));
    assert_ne!(base, image_tag("debian", &install("apk add python3")));
    assert_ne!(base, image_tag("alpine", &install("apk add python3 ")));
    assert_ne!(
        base,
        image_tag("alpine", &Install {
            script: "apk add python3".to_owned(),
            run_as: "root".to_owned(),
        })
    );
}

/// Concatenation must not collide: a longer base with a shorter script has to
/// hash differently from the reverse split of the same bytes.
#[test]
fn the_tag_separates_its_inputs() {
    assert_ne!(
        image_tag("alpine", &install("x")),
        image_tag("alpinex", &install(""))
    );
}

#[test]
fn the_tag_is_a_valid_image_reference() {
    let tag = image_tag("alpine", &install("apk add python3"));

    let (name, hex) = tag.split_once(':').expect("tag has a name and a version");
    assert_eq!(name, "jp-bash");
    assert_eq!(hex.len(), 12);
    assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
}

/// The script is copied in rather than inlined, so a multi-line script needs no
/// escaping, and runs under `sh` because the base need not carry `bash` until
/// the script has installed it.
#[test]
fn the_dockerfile_installs_as_root_and_drops_back() {
    assert_eq!(
        dockerfile("alpine:3.24", "nonroot"),
        "FROM alpine:3.24\nUSER root\nCOPY install.sh /tmp/jp-install.sh\nRUN sh \
         /tmp/jp-install.sh && rm /tmp/jp-install.sh\nUSER nonroot\n"
    );
}

/// A failing command must fail the build rather than bake a half-installed
/// image that misbehaves on every later call.
#[test]
fn the_install_script_runs_under_strict_mode() {
    assert_eq!(
        install_sh("apk add --no-cache python3\npython3 -V\n"),
        "set -eu\napk add --no-cache python3\npython3 -V\n"
    );
}

/// No install script means no build and no runtime interaction at all: the base
/// image is used as it comes.
#[test]
fn no_install_script_uses_the_base_image_untouched() {
    let dir = camino_tempfile::tempdir().unwrap();
    let runner = MockProcessRunner::never_called();

    let image = ensure_image(Runtime::Docker, "alpine", None, dir.path(), &runner).unwrap();

    assert_eq!(image, "alpine");
}

/// A cached image is reused.
/// The tag is content-addressed, so finding it is proof the base and script
/// have not changed since it was built.
#[test]
fn an_existing_image_is_not_rebuilt() {
    let dir = camino_tempfile::tempdir().unwrap();
    let install = install("apk add --no-cache python3");
    let tag = image_tag("alpine", &install);

    let runner = MockProcessRunner::builder()
        .expect("docker")
        .args(&["image", "inspect", &tag])
        .returns_success("[{}]");

    let image = ensure_image(
        Runtime::Docker,
        "alpine",
        Some(&install),
        dir.path(),
        &runner,
    )
    .unwrap();

    assert_eq!(image, tag);
}

#[test]
fn a_missing_image_is_built_and_then_used() {
    let dir = camino_tempfile::tempdir().unwrap();
    let install = install("apk add --no-cache python3");
    let tag = image_tag("alpine", &install);

    let runner = MockProcessRunner::builder()
        .expect("docker")
        .args(&["image", "inspect", &tag])
        .returns_error("no such image")
        .expect("docker")
        .returns_success("built");

    let image = ensure_image(
        Runtime::Docker,
        "alpine",
        Some(&install),
        dir.path(),
        &runner,
    )
    .unwrap();

    assert_eq!(image, tag);
}

/// A failed build must surface the runtime's output: "the build failed" alone
/// leaves the operator with no way to find which command in their script broke.
#[test]
fn a_failed_build_reports_the_runtime_output() {
    let dir = camino_tempfile::tempdir().unwrap();
    let install = install("apk add --no-cache nosuchpackage");

    let runner = MockProcessRunner::builder()
        .expect("docker")
        .returns_error("no such image")
        .expect("docker")
        .returns_error("ERROR: unable to select packages: nosuchpackage (no such package)");

    let error = ensure_image(
        Runtime::Docker,
        "alpine",
        Some(&install),
        dir.path(),
        &runner,
    )
    .unwrap_err()
    .to_string();

    assert!(
        error.starts_with("Failed to build image 'jp-bash:"),
        "got: {error}"
    );
    assert!(
        error.ends_with("nosuchpackage (no such package)"),
        "got: {error}"
    );
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
