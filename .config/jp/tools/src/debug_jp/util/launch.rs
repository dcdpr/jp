//! Spawn `jp` and let the caller attach sidecars (like `sample(1)`) before
//! waiting on it.
//!
//! Two-phase: [`spawn`] returns a [`Handle`] carrying the PID and start time,
//! and the caller calls [`Handle::wait`] when ready.
//! This shape lets each profiling tool start its own profiler keyed on the PID
//! between the two phases.

use std::{
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use camino::Utf8PathBuf;

use crate::Error;

/// What process to launch.
#[derive(Debug, Clone)]
pub(crate) struct LaunchSpec {
    pub binary: Utf8PathBuf,
    pub args: Vec<String>,
    pub working_dir: Utf8PathBuf,
    pub env: Vec<(String, String)>,
}

/// Result of a completed launch.
#[derive(Debug)]
pub(crate) struct LaunchResult {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub wall_duration: Duration,
}

impl LaunchResult {
    pub(crate) fn success(&self) -> bool {
        matches!(self.exit_code, Some(0))
    }
}

/// In-flight launch.
/// Holds the spawned child plus its identifying metadata.
pub(crate) struct Handle {
    child: Child,
    pid: u32,
    started_at: Instant,
}

impl Handle {
    /// PID of the launched process — for attaching profilers like `sample(1)`.
    pub(crate) fn pid(&self) -> u32 {
        self.pid
    }

    /// Wait for the child to exit and collect its output.
    pub(crate) fn wait(self) -> Result<LaunchResult, Error> {
        let started_at = self.started_at;
        let output = self
            .child
            .wait_with_output()
            .map_err(|e| format!("Failed to wait on jp: {e}"))?;
        Ok(LaunchResult {
            exit_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            wall_duration: started_at.elapsed(),
        })
    }
}

/// Spawn the process described by `spec`.
/// Stdout and stderr are captured.
pub(crate) fn spawn(spec: &LaunchSpec) -> Result<Handle, Error> {
    let mut command = Command::new(spec.binary.as_path());
    command
        .args(&spec.args)
        .current_dir(spec.working_dir.as_path())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in &spec.env {
        command.env(key, value);
    }

    let started_at = Instant::now();
    let child = command
        .spawn()
        .map_err(|e| format!("Failed to spawn {}: {e}", spec.binary))?;
    let pid = child.id();
    Ok(Handle {
        child,
        pid,
        started_at,
    })
}
