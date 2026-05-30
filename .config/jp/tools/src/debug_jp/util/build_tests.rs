use std::fs;

use super::*;
use crate::util::runner::MockProcessRunner;

#[test]
fn resolves_binary_path_from_metadata() {
    // `build` only accepts a binary that exists on disk, so lay one down where
    // the (mocked) `cargo metadata` says the target dir is.
    let target = camino_tempfile::tempdir().unwrap();
    let target_dir = target.path();
    fs::create_dir_all(target_dir.join("profiling")).unwrap();
    fs::write(target_dir.join("profiling/jp"), b"").unwrap();

    let metadata_json = format!(r#"{{"target_directory":"{target_dir}"}}"#);
    let runner = MockProcessRunner::builder()
        .expect("cargo") // cargo build
        .returns_success("")
        .expect("cargo") // cargo metadata
        .returns_success(metadata_json);

    let spec = BuildSpec {
        working_dir: target_dir,
        package: "jp_cli",
        bin: "jp",
        profile: "profiling",
        features: &[],
    };

    let binary = build(&runner, &spec).unwrap();
    assert_eq!(binary, target_dir.join("profiling/jp"));
}

#[test]
fn surfaces_cargo_build_failure() {
    let runner = MockProcessRunner::builder()
        .expect("cargo")
        .returns_error("error[E0599]: no method named ...");

    let spec = BuildSpec {
        working_dir: Utf8Path::new("/"),
        package: "jp_cli",
        bin: "jp",
        profile: "profiling",
        features: &[],
    };

    let error = build(&runner, &spec).unwrap_err().to_string();
    assert!(error.contains("`cargo build` failed"), "got: {error}");
}
