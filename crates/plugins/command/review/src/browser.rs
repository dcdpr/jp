//! Opening a page of `jp review serve` in the user's default browser, behind a
//! trait so the commands that do it can be tested without launching one.

use std::{
    env,
    process::{Child, Command, Stdio},
    sync::mpsc::{self, RecvTimeoutError},
    thread,
    time::Duration,
};

/// Maximum time to wait for the browser opener command to exit before assuming
/// it succeeded.
///
/// On Linux, `xdg-open` runs a newly launched browser in the foreground and
/// only exits when the browser closes; if the process is still running after
/// this grace period, it is left running in the background.
const GRACE: Duration = Duration::from_secs(2);

/// Opens a URL in a browser.
pub trait Browser {
    /// Open `url`, returning once the browser has been asked to.
    fn open(&self, url: &str) -> Result<(), String>;
}

/// The user's default browser, which the operating system picks.
pub struct SystemBrowser;

impl Browser for SystemBrowser {
    /// Spawns the platform opener with stdio redirected to `/dev/null`
    /// (preserving stdout for the host plugin protocol) and waits up to
    /// [`GRACE`] for it to exit.
    fn open(&self, url: &str) -> Result<(), String> {
        let (program, args) = opener(env::consts::OS, url).ok_or_else(|| {
            format!(
                "`jp review` can't open a browser on {}; open {url} in one.",
                env::consts::OS
            )
        })?;

        let child = Command::new(program)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("Couldn't run `{program}` to open {url}: {error}"))?;

        settle(child, GRACE).map_err(|why| {
            format!("`{program}` couldn't open {url} ({why}); open it in a browser.")
        })
    }
}

/// Wait up to `grace` for `child` to exit, returning an error if it exits
/// non-zero within that window.
///
/// If `child` is still running after `grace`, a background thread reaps it when
/// it eventually exits so it does not become a zombie process.
fn settle(mut child: Child, grace: Duration) -> Result<(), String> {
    let (sender, exited) = mpsc::channel();
    thread::spawn(move || {
        // Receiver is dropped if `grace` expires first.
        drop(sender.send(child.wait()));
    });

    match exited.recv_timeout(grace) {
        Ok(Ok(status)) if status.success() => Ok(()),
        Ok(Ok(status)) => Err(status.to_string()),
        Ok(Err(error)) => Err(error.to_string()),
        Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => Ok(()),
    }
}

/// The program, and its arguments, that opens `url` in the default browser on
/// the operating system `os`, as `std::env::consts::OS` names it; `None` where
/// there is none to run.
///
/// On Windows the URL goes to the URL protocol handler directly, which `cmd`
/// would otherwise split at `&`.
pub fn opener(os: &str, url: &str) -> Option<(&'static str, Vec<String>)> {
    match os {
        "macos" => Some(("open", vec![url.to_owned()])),
        "windows" => Some(("rundll32", vec![
            "url.dll,FileProtocolHandler".to_owned(),
            url.to_owned(),
        ])),
        "linux" | "freebsd" | "netbsd" | "openbsd" | "dragonfly" | "illumos" | "solaris" => {
            Some(("xdg-open", vec![url.to_owned()]))
        }
        _ => None,
    }
}

#[cfg(test)]
#[path = "browser_tests.rs"]
mod tests;
