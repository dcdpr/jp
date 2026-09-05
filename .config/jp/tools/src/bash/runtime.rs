//! Container runtime selection, image preparation, and command-line rendering.
//!
//! [`detect`] picks the first OCI-compatible runtime found on `PATH`.
//! [`ensure_image`] resolves the image a call runs in, building one when the
//! tool config supplies an install script.
//! [`argv`] turns a [`RunSpec`] into the program and arguments that run it.
//!
//! Detection and image building touch the host; tag computation, Dockerfile
//! text, and argv rendering are pure, so what a call produces is testable
//! without any runtime installed.

use std::{fmt::Write as _, fs};

use camino::{Utf8Path, Utf8PathBuf};
use sha2::{Digest, Sha256};

use crate::{
    Error,
    util::{runner::ProcessRunner, truncate},
};

/// Truncate build output beyond this limit when reporting a build failure.
const MAX_BUILD_LOG_BYTES: usize = 20_000;

/// An OCI-compatible container runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Runtime {
    /// Apple's `container`, which needs macOS 26 on Apple silicon.
    Apple,
    Docker,
    Podman,
}

impl Runtime {
    /// The executable name to invoke.
    pub(super) const fn program(self) -> &'static str {
        match self {
            Self::Apple => "container",
            Self::Docker => "docker",
            Self::Podman => "podman",
        }
    }
}

/// Runtimes to look for, in preference order.
const CANDIDATES: &[Runtime] = &[Runtime::Apple, Runtime::Docker, Runtime::Podman];

/// A base image plus the script that mutates it.
///
/// The pair is content-addressed into a tag by [`image_tag`], so an image is
/// built once and reused until either half changes.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Install {
    /// Shell script run as root while building the image.
    pub script: String,

    /// User the container runs as once the script has finished.
    pub run_as: String,
}

/// One throwaway container run.
#[derive(Debug)]
pub(super) struct RunSpec {
    /// Image reference to run.
    pub image: String,

    /// Host path and container path for each read-only bind mount.
    pub mounts: Vec<(Utf8PathBuf, String)>,

    /// Names of environment variables to forward into the container.
    ///
    /// Values are taken from the environment of the runtime process, so the
    /// caller must set them there.
    pub envs: Vec<String>,

    /// Working directory inside the container.
    pub workdir: Option<String>,

    /// The script handed to `bash -c`.
    pub script: String,
}

/// The first runtime found on `PATH`, in [`CANDIDATES`] order.
pub(super) fn detect() -> Option<Runtime> {
    CANDIDATES
        .iter()
        .copied()
        .find(|runtime| which::which(runtime.program()).is_ok())
}

/// The tag identifying `base` mutated by `install`.
///
/// Content-addressed, so the same base and script always name the same image
/// and a built one is reused.
/// Editing the script — a comment counts — produces a different tag and
/// forces a rebuild, which is the only way to pick up newer package versions.
pub(super) fn image_tag(base: &str, install: &Install) -> String {
    let mut hasher = Sha256::new();
    // Separators keep ("ab", "c") from hashing the same as ("a", "bc").
    for part in [base, install.script.as_str(), install.run_as.as_str()] {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }

    let digest = hasher.finalize();
    let mut hex = String::with_capacity(12);
    for byte in &digest[..6] {
        write!(hex, "{byte:02x}").expect("writing to a String cannot fail");
    }

    format!("jp-bash:{hex}")
}

/// The Dockerfile that applies an install script to `base`.
///
/// The script is copied in rather than inlined so a multi-line script needs no
/// escaping, and is run with `sh` because the base is not guaranteed to carry
/// `bash` before the script has run.
pub(super) fn dockerfile(base: &str, run_as: &str) -> String {
    format!(
        "FROM {base}\nUSER root\nCOPY install.sh /tmp/jp-install.sh\nRUN sh /tmp/jp-install.sh && \
         rm /tmp/jp-install.sh\nUSER {run_as}\n"
    )
}

/// Wrap an operator's install script for the build.
///
/// `set -eu` makes a failing command fail the build, rather than baking a
/// half-installed image that misbehaves on every later call.
pub(super) fn install_sh(script: &str) -> String {
    format!("set -eu\n{}\n", script.trim_end())
}

/// The image a call should run in, building it when `install` is set.
///
/// Returns `base` unchanged when there is nothing to install.
/// Otherwise returns the content-addressed tag, building it first if the
/// runtime does not already have it.
///
/// # Errors
///
/// Returns an error when the build context cannot be written, the runtime
/// cannot be invoked, or the build itself fails; a build failure carries the
/// runtime's output so the operator can see which command broke.
pub(super) fn ensure_image<R: ProcessRunner>(
    runtime: Runtime,
    base: &str,
    install: Option<&Install>,
    workdir: &Utf8Path,
    runner: &R,
) -> Result<String, Error> {
    let Some(install) = install else {
        return Ok(base.to_owned());
    };

    let tag = image_tag(base, install);
    let program = runtime.program();

    if runner
        .run(program, &["image", "inspect", &tag], workdir)?
        .success()
    {
        return Ok(tag);
    }

    // The build can take minutes on a cold base, and the tool produces no
    // output until it finishes. Say so on stderr, which the host forwards to
    // its trace, so the wait doesn't read as a hang.
    eprintln!("jp bash: building {tag} from {base}");

    let context = std::env::temp_dir().join(tag.replace(':', "-"));
    fs::create_dir_all(&context)?;
    fs::write(
        context.join("Dockerfile"),
        dockerfile(base, &install.run_as),
    )?;
    fs::write(context.join("install.sh"), install_sh(&install.script))?;

    let context_arg = context.to_string_lossy().into_owned();
    let build = runner.run(program, &["build", "-t", &tag, &context_arg], workdir);
    drop(fs::remove_dir_all(&context));

    let build = build?;
    if !build.success() {
        let log = truncate(
            format!("{}\n{}", build.stdout, build.stderr).trim(),
            MAX_BUILD_LOG_BYTES,
        );
        return Err(format!("Failed to build image '{tag}' from '{base}':\n{log}").into());
    }

    Ok(tag)
}

/// Render the command line that runs `spec` under `runtime`.
///
/// Environment variables are forwarded by name (`--env NAME`), so their values
/// stay out of the argument vector and out of `ps` output.
/// Every mount is read-only, and `--rm` removes the container when the script
/// exits.
pub(super) fn argv(runtime: Runtime, spec: &RunSpec) -> (String, Vec<String>) {
    let mut args = vec!["run".to_owned(), "--rm".to_owned()];

    for (host, target) in &spec.mounts {
        args.push("--volume".to_owned());
        args.push(format!("{host}:{target}:ro"));
    }

    for name in &spec.envs {
        args.push("--env".to_owned());
        args.push(name.clone());
    }

    if let Some(workdir) = &spec.workdir {
        args.push("--workdir".to_owned());
        args.push(workdir.clone());
    }

    args.push(spec.image.clone());
    args.extend(["bash".to_owned(), "-c".to_owned(), spec.script.clone()]);

    (runtime.program().to_owned(), args)
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
