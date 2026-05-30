//! Build the `jp` binary for a profiling run.
//!
//! Always invokes `cargo build` from the sandbox working directory.
//! Cargo walks up to the project root's `.cargo/config.toml` for the shared
//! `target/`, so incremental builds across the user's worktree and the sandbox
//! share artifacts.

use camino::{Utf8Path, Utf8PathBuf};

use crate::{Error, util::runner::ProcessRunner};

/// What to build and how.
#[derive(Debug, Clone)]
pub(crate) struct BuildSpec<'a> {
    /// Working directory `cargo build` runs from.
    pub working_dir: &'a Utf8Path,

    /// Cargo package name, e.g. `jp_cli`.
    pub package: &'a str,

    /// Binary name produced by the package, e.g. `jp`.
    pub bin: &'a str,

    /// Cargo profile to build with, e.g. `profiling`.
    pub profile: &'a str,

    /// Feature flags to enable on the package.
    pub features: &'a [&'a str],
}

/// Build `jp` and return the path to the resulting binary.
pub(crate) fn build(
    runner: &dyn ProcessRunner,
    spec: &BuildSpec<'_>,
) -> Result<Utf8PathBuf, Error> {
    let mut args = vec![
        "build".to_owned(),
        format!("--package={}", spec.package),
        format!("--bin={}", spec.bin),
        format!("--profile={}", spec.profile),
        "--quiet".to_owned(),
    ];
    if !spec.features.is_empty() {
        args.push(format!("--features={}", spec.features.join(",")));
    }
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();

    let output = runner
        .run("cargo", &arg_refs, spec.working_dir)
        .map_err(|e| format!("Failed to spawn `cargo build`: {e}"))?;
    if !output.success() {
        return Err(format!("`cargo build` failed: {}", output.stderr).into());
    }

    // Resolve the binary path via `cargo metadata` so we honor the
    // workspace's `target/` layout from `.cargo/config.toml`. Both
    // worktrees share the same target dir, so the artifact ends up where
    // any other cargo build in this workspace would put it.
    let metadata = runner
        .run(
            "cargo",
            &["metadata", "--no-deps", "--format-version=1"],
            spec.working_dir,
        )
        .map_err(|e| format!("Failed to spawn `cargo metadata`: {e}"))?;
    if !metadata.success() {
        return Err(format!("`cargo metadata` failed: {}", metadata.stderr).into());
    }

    let target_dir = metadata
        .stdout
        .split_once("\"target_directory\":\"")
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(path, _)| Utf8PathBuf::from(path))
        .ok_or_else(|| "Failed to parse `target_directory` from cargo metadata".to_owned())?;

    let binary = target_dir.join(spec.profile).join(spec.bin);
    if !binary.exists() {
        return Err(format!("Build succeeded but binary not found at {binary}").into());
    }
    Ok(binary)
}

#[cfg(test)]
#[path = "build_tests.rs"]
mod tests;
