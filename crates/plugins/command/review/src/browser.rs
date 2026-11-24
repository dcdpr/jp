//! Opening a page of `jp review serve` in the user's default browser, behind a
//! trait so the commands that do it can be tested without launching one.

use std::{
    env,
    process::{Command, Stdio},
};

/// Opens a URL in a browser.
pub trait Browser {
    /// Open `url`, returning once the browser has been asked to.
    fn open(&self, url: &str) -> Result<(), String>;
}

/// The user's default browser, which the operating system picks.
pub struct SystemBrowser;

impl Browser for SystemBrowser {
    /// The command runs with its output discarded: this process's stdout is its
    /// connection to the host.
    fn open(&self, url: &str) -> Result<(), String> {
        let (program, args) = opener(env::consts::OS, url).ok_or_else(|| {
            format!(
                "`jp review` can't open a browser on {}; open {url} in one.",
                env::consts::OS
            )
        })?;

        let status = Command::new(program)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|error| format!("Couldn't run `{program}` to open {url}: {error}"))?;
        if status.success() {
            return Ok(());
        }

        Err(format!(
            "`{program}` couldn't open {url} ({status}); open it in a browser."
        ))
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
