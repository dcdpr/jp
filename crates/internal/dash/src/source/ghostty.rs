//! Ghostty's terminals, through its `AppleScript` dictionary.
//!
//! Ghostty (1.3 and later) exposes every terminal surface with its ID and
//! working directory, can focus one by ID, and can open a tab in a directory.
//! Scripting it needs the macOS Automation permission, which the system asks
//! for on first use.

use std::process::Command;

use camino::Utf8Path;
use serde::Deserialize;

use super::{Error, canonical, stdout};
use crate::model::Terminal;

/// The title the dashboard gives its own terminal, so a new tab can be opened
/// in some other window than the dashboard's.
pub const TITLE: &str = "jp dash";

/// Opens a tab in the directory `item 1 of argv`, in the first window that
/// holds no terminal titled `item 2 of argv`, or in a new window when every
/// window does.
/// The tab's shell starts with `item 3 of argv` typed at its prompt, unless
/// that is empty.
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

/// Focuses the terminal with ID `item 1 of argv`, pastes `item 2 of argv` at
/// its prompt, and presses enter.
const RUN_IN: &str = r#"
on run argv
    tell application "Ghostty"
        set t to terminal id (item 1 of argv)
        focus t
        input text (item 2 of argv) to t
        send key "enter" to t
    end tell
end run
"#;

/// Lists every terminal as JSON.
///
/// Checking `running()` first matters: sending any other event to an
/// application that is not running launches it.
/// A build older than the `tty` property throws on reading it, and reports no
/// tty rather than failing the whole list.
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
    // The ID travels as an argument, so no value can change the script.
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

/// Open a Ghostty tab in `dir`, away from the dashboard's own window, and bring
/// it to the front.
/// With `command`, the tab's shell runs it as soon as it starts.
pub fn open_tab(dir: &Utf8Path, command: Option<&str>) -> Result<(), Error> {
    let input = command
        .map(|command| format!("{command}\n"))
        .unwrap_or_default();

    // The directory and command travel as arguments, so no value can change
    // the script.
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

/// Focus the terminal with ID `id`, and run `command` at its shell's prompt, as
/// if typed there.
///
/// Whatever program is in the terminal's foreground gets the input, so the
/// caller makes sure that is the shell.
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
/// A terminal whose shell never reported a working directory cannot be matched
/// to a worktree, and is left out.
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

/// A terminal device's name as `ps` prints it (`ttys001`), from the path
/// Ghostty reports (`/dev/ttys001`).
/// `None` for no device.
#[must_use]
pub fn tty_name(path: &str) -> Option<String> {
    let name = path.trim().trim_start_matches("/dev/");
    (!name.is_empty() && name != "??").then(|| name.to_owned())
}

#[cfg(test)]
#[path = "ghostty_tests.rs"]
mod tests;
