//! Container runtime selection and command-line rendering.
//!
//! [`detect`] picks the first OCI-compatible runtime found on `PATH`.
//! [`argv`] turns a [`RunSpec`] into the program and arguments that run it.
//!
//! Only detection touches the host; rendering is pure, so the exact command
//! line a call produces is testable without any runtime installed.

use camino::Utf8PathBuf;

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
