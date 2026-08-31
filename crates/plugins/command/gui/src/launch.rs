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

/// Why `target_os` cannot open the app, or `None` if it can.
///
/// The plugin builds and publishes for every target the release workflow
/// covers, because the registry has no way to say a plugin is macOS-only.
/// So a Linux or Windows user can install it, and what they get should name the
/// reason rather than fail reaching for a `macos` binary that was never going
/// to be there.
pub(crate) fn unsupported_platform(target_os: &str) -> Option<String> {
    if target_os == "macos" {
        return None;
    }

    Some(format!(
        "`jp gui` opens the JP macOS app, which does not run on {target_os}."
    ))
}

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
        if let Some(reason) = unsupported_platform(std::env::consts::OS) {
            return Err(reason);
        }

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
