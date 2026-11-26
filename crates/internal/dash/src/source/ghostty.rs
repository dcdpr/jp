//! Ghostty terminal discovery and control via `osascript`.
//!
//! Ghostty 1.3+ exposes open terminal surfaces (with their ID, working
//! directory, title, and TTY) over `AppleScript` / JXA, and supports focusing
//! terminals and opening tabs.
//! Scripting Ghostty requires the macOS Automation permission, which the OS
//! prompts for on first use.

use std::process::Command;

use camino::Utf8Path;
use serde::Deserialize;

use super::{Error, canonical, stdout};
use crate::model::Terminal;

/// Terminal title set by the dashboard so new tabs are opened in a different
/// Ghostty window.
pub const TITLE: &str = "jp dash";

/// `AppleScript` that opens a tab in `item 1 of argv` inside the first Ghostty
/// window containing no terminal whose title contains `item 2 of argv` (or in a
/// new window if none qualifies).
/// If `item 3 of argv` is non-empty, sends it as the initial input to the new
/// surface.
const OPEN_TAB: &str = r#"
on run argv
    tell application "Ghostty"
        set cfg to new surface configuration
        set initial working directory of cfg to (item 1 of argv)
        if (item 3 of argv) is not "" then
            set initial input of cfg to (item 3 of argv)
        end if

        set target to missing value
        repeat with candidate in windows
            if (count of (terminals of candidate whose name contains (item 2 of argv))) = 0 then
                set target to contents of candidate
                exit repeat
            end if
        end repeat

        if target is missing value then
            new window with configuration cfg
        else
            new tab in target with configuration cfg
            activate window target
        end if
        activate
    end tell
end run
"#;

/// `AppleScript` that focuses terminal `item 1 of argv`, sends `Ctrl-C` to
/// clear any partial input at the prompt, types `item 2 of argv`, and presses
/// `Enter`.
///
/// Without `Ctrl-C`, the command would be appended to any text already typed at
/// the prompt.
const RUN_IN: &str = r#"
on run argv
    tell application "Ghostty"
        set t to terminal id (item 1 of argv)
        focus t
        send key "c" modifiers "control" to t
        input text (item 2 of argv) to t
        send key "enter" to t
    end tell
end run
"#;

/// JXA script that serializes all open Ghostty terminals to JSON.
///
/// Checks `app.running()` first because sending Apple Events to a non-running
/// application launches it.
/// Wraps `t.tty()` in a `try` block so Ghostty builds without the `tty`
/// property return `""` instead of failing the query.
const LIST_TERMINALS: &str = r#"
const app = Application("Ghostty");
const tty = t => { try { return t.tty(); } catch (e) { return ""; } };
app.running()
    ? JSON.stringify(app.terminals().map(t => ({ id: t.id(), cwd: t.workingDirectory(), name: t.name(), tty: tty(t) })))
    : "[]";
"#;

/// List Ghostty's terminals, or none when Ghostty is not running.
pub fn terminals() -> Result<Vec<Terminal>, Error> {
    let json = stdout(
        "osascript",
        Command::new("osascript").args(["-l", "JavaScript", "-e", LIST_TERMINALS]),
    )?;

    Ok(parse(&json)?
        .into_iter()
        .map(|terminal| Terminal {
            cwd: canonical(terminal.cwd.as_str()),
            ..terminal
        })
        .collect())
}

/// Focus the terminal with ID `id`, bringing its window to the front.
pub fn focus(id: &str) -> Result<(), Error> {
    // Pass dynamic values via `argv` rather than interpolating them into the script.
    stdout(
        "osascript",
        Command::new("osascript")
            .args(["-e", "on run argv"])
            .args([
                "-e",
                "tell application \"Ghostty\" to focus terminal id (item 1 of argv)",
            ])
            .args(["-e", "end run"])
            .arg(id),
    )
    .map(drop)
}

/// Open a Ghostty tab in `dir` (in a window other than the dashboard's) and
/// bring it to the front, optionally running `command` on startup.
pub fn open_tab(dir: &Utf8Path, command: Option<&str>) -> Result<(), Error> {
    let input = command.map_or_default(|command| format!("{command}\n"));

    // Pass dynamic values via `argv` rather than interpolating them into the script.
    stdout(
        "osascript",
        Command::new("osascript")
            .args(["-e", OPEN_TAB])
            .arg(dir)
            .arg(TITLE)
            .arg(input),
    )
    .map(drop)
}

/// Focus terminal `id`, clear any partial prompt input with `Ctrl-C`, and run
/// `command`.
///
/// Input is sent to whatever process is in the terminal's foreground group; the
/// caller must verify that the shell is idle before calling this function.
pub fn run_in(id: &str, command: &str) -> Result<(), Error> {
    stdout(
        "osascript",
        Command::new("osascript")
            .args(["-e", RUN_IN])
            .arg(id)
            .arg(command),
    )
    .map(drop)
}

#[derive(Deserialize)]
struct RawTerminal {
    id: String,
    cwd: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    tty: Option<String>,
}

/// Parse the output of [`LIST_TERMINALS`].
///
/// Terminals without a reported working directory cannot be matched to a
/// worktree and are omitted.
fn parse(json: &str) -> Result<Vec<Terminal>, Error> {
    let raw: Vec<RawTerminal> = serde_json::from_str(json).map_err(|source| Error::Parse {
        program: "osascript",
        source,
    })?;

    Ok(raw
        .into_iter()
        .filter_map(|terminal| {
            let cwd = terminal.cwd.filter(|cwd| !cwd.is_empty())?;
            Some(Terminal {
                id: terminal.id,
                cwd: cwd.into(),
                name: terminal.name.unwrap_or_default(),
                tty: terminal.tty.as_deref().and_then(tty_name),
            })
        })
        .collect())
}

/// Normalize a TTY path (such as `/dev/ttys001`) to the short device name
/// printed by `ps` (`ttys001`).
///
/// Returns `None` if `path` is empty or `??`.
#[must_use]
pub fn tty_name(path: &str) -> Option<String> {
    let name = path.trim().trim_start_matches("/dev/");
    (!name.is_empty() && name != "??").then(|| name.to_owned())
}

#[cfg(test)]
#[path = "ghostty_tests.rs"]
mod tests;
