//! Launching the app, behind a trait so the rest can be tested without opening
//! a window.

use std::process::Command;

/// Starts the app on a workspace.
pub(crate) trait Launcher {
    /// Open the app identified by `bundle_id`, showing the workspace at `path`.
    fn launch(&self, bundle_id: &str, path: &str) -> Result<(), String>;
}

/// Launches through macOS Launch Services.
pub(crate) struct SystemLauncher;

impl Launcher for SystemLauncher {
    /// Hands the workspace over as `JP_WORKSPACE`, which is what the app reads
    /// when a window opens with no workspace of its own.
    ///
    /// `open -b` finds the app by bundle identifier, so nothing here depends on
    /// where it was installed.
    /// `-n` is deliberately absent: a second `jp gui` for a workspace already
    /// on screen should bring that window forward rather than start a second
    /// copy of the app.
    fn launch(&self, bundle_id: &str, path: &str) -> Result<(), String> {
        let status = Command::new("open")
            .arg("-b")
            .arg(bundle_id)
            .arg("--env")
            .arg(format!("JP_WORKSPACE={path}"))
            .status()
            .map_err(|e| format!("could not run `open`: {e}"))?;

        if status.success() {
            return Ok(());
        }

        Err(format!(
            "could not open the JP app (`open` exited with {status}). Build and install it with \
             `just build-app`, or open it once by hand so macOS knows where it is."
        ))
    }
}
