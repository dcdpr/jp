//! `jp ticket edit` with nothing to change: the ticket's file, opened in the
//! user's editor, and checked when the editor closes.
//!
//! The editor runs on the host, which owns the terminal ([`Host`]).
//! [`in_editor`] reaches it through [`Editing`], so the checks after an edit
//! can be tested without one.

use std::{
    fs,
    io::{BufRead, Write},
};

use camino::Utf8Path;
use jp_plugin::message::{
    ComposeMode, ComposeOption, ComposeRequest, EditFilesRequest, HostToPlugin, PluginToHost,
};
use ticket::{TicketId, parse, store};

use crate::{Output, compose, options::Options, read_message, send};

/// The protocol that added `edit_files`, through which the host opens the
/// editor.
pub(crate) const EDIT_FILES_PROTOCOL: u32 = 13;

/// What to tell someone who can't get an editor: the flags do the same.
const USE_THE_FLAGS: &str = "Pass what to change instead: --title, --body, --status, or --label.";

/// How an edit reaches the user while the ticket is open.
pub(crate) trait Editing {
    /// Open `path` in the user's editor and wait for it to close.
    ///
    /// `true` when the editor exited cleanly.
    fn open(&mut self, path: &Utf8Path) -> Result<bool, String>;

    /// Ask whether to open the editor again, after `problem` was found in what
    /// it left behind.
    fn again(&mut self, problem: &str) -> bool;
}

/// [`Editing`] through the host: `edit_files` opens the editor, and `compose`
/// asks the question.
pub(crate) struct Host<'a, R, W> {
    pub(crate) stdin: &'a mut R,
    pub(crate) stdout: &'a mut W,
}

impl<R: BufRead, W: Write> Editing for Host<'_, R, W> {
    fn open(&mut self, path: &Utf8Path) -> Result<bool, String> {
        send(
            self.stdout,
            &PluginToHost::EditFiles(EditFilesRequest {
                id: None,
                paths: vec![path.to_owned()],
            }),
        )?;

        match read_message(self.stdin)? {
            HostToPlugin::Edited(response) => Ok(response.saved),
            HostToPlugin::Error(error) => Err(format!(
                "Couldn't open an editor: {}. {USE_THE_FLAGS}",
                error.message
            )),
            HostToPlugin::Shutdown => Err("Interrupted.".to_owned()),
            other => Err(format!("expected an edited response, got: {other:?}")),
        }
    }

    fn again(&mut self, problem: &str) -> bool {
        let choice = compose(self.stdin, self.stdout, ComposeRequest {
            id: None,
            message: problem.to_owned(),
            mode: ComposeMode::Select {
                options: vec![
                    ComposeOption {
                        value: "edit".to_owned(),
                        label: "Open the editor again".to_owned(),
                    },
                    ComposeOption {
                        value: "discard".to_owned(),
                        label: "Discard the edit".to_owned(),
                    },
                ],
                default: Some("edit".to_owned()),
            },
            help: None,
        });

        // No answer, from a cancelled prompt or a missing terminal, discards.
        choice.is_ok_and(|choice| choice == "edit")
    }
}

/// Open ticket `id` in the editor of the host that sent `init`, speaking
/// protocol `version`.
pub(crate) fn through_host(
    version: u32,
    dir: &Utf8Path,
    id: TicketId,
    options: &Options,
    stdin: &mut impl BufRead,
    stdout: &mut impl Write,
) -> Result<Output, String> {
    if version < EDIT_FILES_PROTOCOL {
        return Err(format!(
            "Opening an editor needs `jp` protocol {EDIT_FILES_PROTOCOL}, and this `jp` speaks \
             {version}; reinstall the two together. {USE_THE_FLAGS}"
        ));
    }

    in_editor(dir, id, options, &mut Host { stdin, stdout })
}

/// Open ticket `id` in the user's editor until what comes back is a ticket the
/// board accepts, or the user gives up on the edit.
///
/// The editor writes the file itself.
/// A file that no longer reads as a ticket, or carries a label the board
/// refuses, is offered back to the editor; an edit given up on, or abandoned in
/// the editor, is undone, and so is one the command fails on before it is
/// accepted.
/// A changed title moves the file to the name it produces, as `rename` does.
pub(crate) fn in_editor(
    dir: &Utf8Path,
    id: TicketId,
    options: &Options,
    editing: &mut impl Editing,
) -> Result<Output, String> {
    let path = store::locate_ticket(dir, id).map_err(|error| error.to_string())?;
    let original = read(&path)?;

    let edited = match settle(dir, id, &path, &original, options, editing) {
        Ok(Some(edited)) => edited,
        Ok(None) => return Ok(format!("{path}: unchanged\n").into()),
        // Whatever the editor left is unchecked, so it doesn't stay on disk.
        Err(error) => {
            return Err(match fs::write(&path, &original) {
                Ok(()) => error,
                Err(restore) => format!("{error}\nfailed to restore {path}: {restore}"),
            });
        }
    };

    // Only a changed title moves the file, as with `rename`: a name that
    // already disagreed with the heading is left the way it was found.
    let title = |source: &str| parse::document(source).ok().map(|ticket| ticket.title);
    if title(&edited) == title(&original) {
        return Ok(format!("Edited {path}\n").into());
    }

    let store::Edited { from, to } =
        store::rename_to_title(dir, id).map_err(|error| error.to_string())?;

    Ok(if from == to {
        format!("Edited {to}\n")
    } else {
        format!("Edited {to}, renamed from {from}\n")
    }
    .into())
}

/// Open the editor on `path` until it leaves a ticket the board accepts,
/// returning it, or `None` when the file comes back as `original`.
///
/// An error leaves on disk whatever the editor wrote, for the caller to undo.
fn settle(
    dir: &Utf8Path,
    id: TicketId,
    path: &Utf8Path,
    original: &str,
    options: &Options,
    editing: &mut impl Editing,
) -> Result<Option<String>, String> {
    loop {
        if !editing.open(path)? {
            return Err(format!(
                "The editor exited with an error; {id} is unchanged."
            ));
        }

        let edited = read(path)?;
        if edited == original {
            return Ok(None);
        }

        match check(dir, id, &edited, original, options)? {
            Ok(()) => return Ok(Some(edited)),
            Err(problem) if editing.again(&format!("{id} can't be saved: {problem}")) => {}
            Err(problem) => return Err(format!("{id} is unchanged: {problem}")),
        }
    }
}

fn read(path: &Utf8Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|error| format!("failed to read {path}: {error}"))
}

/// Whether `edited` is ticket `id` as the board accepts it: one that parses,
/// carrying only labels the board takes or that `original` already carried, and
/// none another ticket holds that the board lets one ticket carry.
///
/// The outer error is the board failing to load, which no edit fixes; the inner
/// one is a problem with the edit.
fn check(
    dir: &Utf8Path,
    id: TicketId,
    edited: &str,
    original: &str,
    options: &Options,
) -> Result<Result<(), String>, String> {
    let ticket = match parse::document(edited) {
        Ok(ticket) => ticket,
        Err(problem) => return Ok(Err(problem.to_string())),
    };

    // Labels left as they were don't read the vocabulary, so a board whose
    // vocabulary is broken can still have its descriptions edited.
    let current = parse::labels(original);
    if ticket.metadata.labels == current {
        return Ok(Ok(()));
    }

    let vocabulary = options.vocabulary()?;
    let labels = match vocabulary.resolve_against(&ticket.metadata.labels.to_tokens(), &current) {
        Ok(labels) => labels,
        Err(refusal) => return Ok(Err(refusal.to_string())),
    };

    match store::check_unique(dir, &vocabulary, &labels, Some(id)) {
        Ok(()) => Ok(Ok(())),
        Err(taken @ store::Error::Taken(_)) => Ok(Err(taken.to_string())),
        Err(other) => Err(other.to_string()),
    }
}

#[cfg(test)]
#[path = "edit_tests.rs"]
mod tests;
