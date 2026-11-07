//! `jp release <type>` and `jp release highlight`: a fragment or highlight,
//! opened in the user's editor and checked when the editor closes.
//!
//! The editor runs on the host, which owns the terminal ([`Host`]).
//! [`fragment`] and [`highlight`] reach it through [`Editing`], so what happens
//! after an edit can be tested without one.
//!
//! A file that doesn't follow its format is offered back to the editor, and one
//! given up on is undone: a new file is removed, and an existing one restored.
//! A file saved empty is removed.
//! An accepted file is laid out the way the workspace formats the fragments
//! directory.
//!
//! A new highlight is written in a draft outside the fragments directory, and
//! filed under the slug its title gives once it follows the format.

use std::{
    fs,
    io::{self, BufRead, Write},
};

use camino::{Utf8Path, Utf8PathBuf};
use changelog::{Type, fragment, highlight};
use jp_plugin::message::{
    ComposeMode, ComposeOption, ComposeRequest, EditFilesRequest, HostToPlugin, PluginToHost,
};

use crate::{
    Output, compose,
    entry::{self, Fragments, HIGHLIGHTS, Planned},
    read_message,
    repository::temporary,
    send,
};

/// The protocol that added `edit_files`, through which the host opens the
/// editor.
pub(crate) const EDIT_FILES_PROTOCOL: u32 = 13;

/// How an edit reaches the user.
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
            HostToPlugin::Error(error) => {
                Err(format!("couldn't open an editor: {}", error.message))
            }
            HostToPlugin::Shutdown => Err("interrupted".to_owned()),
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

/// Refuse a host, speaking protocol `version`, that can't open an editor.
pub(crate) fn check_host(version: u32) -> Result<(), String> {
    if version >= EDIT_FILES_PROTOCOL {
        return Ok(());
    }

    Err(format!(
        "Opening an editor needs `jp` protocol {EDIT_FILES_PROTOCOL}, and this `jp` speaks \
         {version}; reinstall the two together."
    ))
}

/// Open the fragment of type `type_` named `slug` in the editor, a new and
/// empty one when there is none, and keep what the editor leaves once it
/// follows the format.
pub(crate) fn fragment(
    fragments: &Fragments<'_>,
    type_: Type,
    slug: &str,
    editing: &mut impl Editing,
) -> Result<Output, String> {
    entry::ready(fragments)?;
    entry::check_slug(slug)?;

    let parse = |text: &str| {
        fragment::parse(type_, text)
            .map(drop)
            .map_err(|error| error.to_string())
    };

    in_place(
        fragments,
        &fragments.fragment(slug, type_),
        "",
        parse,
        editing,
    )
}

/// Open the highlight named `slug` in the editor, or a new one named after its
/// title when there is no `slug`, and keep what the editor leaves once it
/// follows the format.
///
/// A new highlight opens with a title to write, and `author` credited.
pub(crate) fn highlight(
    fragments: &Fragments<'_>,
    slug: Option<&str>,
    author: Option<&str>,
    editing: &mut impl Editing,
) -> Result<Output, String> {
    entry::ready(fragments)?;

    let template = format!("# \n\n- **Authors**: {}\n\n", author.unwrap_or_default());
    let parse = |text: &str| {
        highlight::parse(text)
            .map(drop)
            .map_err(|error| error.to_string())
    };

    if let Some(slug) = slug {
        entry::check_slug(slug)?;
        return in_place(
            fragments,
            &fragments.highlight(slug),
            &template,
            parse,
            editing,
        );
    }

    drafted(fragments, &template, parse, editing)
}

/// How an edit ended.
enum Ended {
    /// The editor left text that follows the format.
    Accepted(String),

    /// The editor left the file as it was opened.
    Unchanged,

    /// The editor left nothing but whitespace.
    Emptied,
}

/// Open `path`, created holding `initial` when it isn't there, until what the
/// editor leaves follows the format `parse` holds it to, or the edit is given
/// up on.
fn in_place(
    fragments: &Fragments<'_>,
    path: &Utf8Path,
    initial: &str,
    parse: impl Fn(&str) -> Result<(), String>,
    editing: &mut impl Editing,
) -> Result<Output, String> {
    let shown = fragments.show(path);
    let original = match fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("failed to read {shown}: {error}")),
    };
    if original.is_none() {
        create(path, &shown, initial)?;
    }

    let opened = original.as_deref().unwrap_or(initial);
    let check = |text: &str| checked(fragments, path, text, &parse);
    let ended = settle(editing, path, &shown, opened, &check);

    let Some(original) = original else {
        // A new file stays only once the editor leaves one that follows the
        // format.
        return match ended {
            Ok(Ended::Accepted(text)) => {
                lay_out(fragments, path, &shown, &text)?;
                Ok(format!("Wrote {shown}\n").into())
            }
            Ok(Ended::Unchanged | Ended::Emptied) => {
                remove(path, &shown)?;
                Ok("Nothing written.\n".to_owned().into())
            }
            Err(problem) => Err(undone(
                remove(path, &shown),
                format!("Nothing written: {problem}"),
            )),
        };
    };

    match ended {
        Ok(Ended::Accepted(text)) => {
            lay_out(fragments, path, &shown, &text)?;
            Ok(format!("Edited {shown}\n").into())
        }
        Ok(Ended::Unchanged) => match check(&original) {
            Ok(()) => Ok(format!("{shown}: unchanged\n").into()),
            Err(problem) => Err(format!(
                "{shown} is unchanged, and still breaks the format: {problem}"
            )),
        },
        Ok(Ended::Emptied) => {
            remove(path, &shown)?;
            Ok(format!("Removed {shown}, which was saved empty.\n").into())
        }
        Err(problem) => {
            let restored = fs::write(path, &original)
                .map_err(|error| format!("failed to restore {shown}: {error}"));
            Err(undone(restored, format!("{shown} is unchanged: {problem}")))
        }
    }
}

/// Write a new highlight in a draft outside the fragments directory, opened
/// holding `template`, and file it under the slug its title gives once it
/// follows the format `parse` holds it to.
fn drafted(
    fragments: &Fragments<'_>,
    template: &str,
    parse: impl Fn(&str) -> Result<(), String>,
    editing: &mut impl Editing,
) -> Result<Output, String> {
    const SHOWN: &str = "The new highlight";

    let draft = Utf8PathBuf::from(format!("{}.md", temporary("highlight")?));
    create(&draft, SHOWN, template)?;

    // Laid out as any highlight is, whatever it ends up named.
    let filed_as = fragments.dir.join(HIGHLIGHTS).join("new.md");
    let check = |text: &str| checked(fragments, &filed_as, text, &parse);
    let outcome = match settle(editing, &draft, SHOWN, template, &check) {
        Ok(Ended::Accepted(text)) => file_highlight(fragments, &text),
        Ok(Ended::Unchanged | Ended::Emptied) => Ok("Nothing written.\n".to_owned().into()),
        Err(problem) => Err(format!("Nothing written: {problem}")),
    };

    // Filed or given up on, the draft has served its purpose.
    drop(fs::remove_file(&draft));

    outcome
}

/// File the highlight `text` in `fragments` under the slug its title gives,
/// laid out as the workspace formats highlights.
///
/// A slug another highlight has gets `-2`, `-3`, and so on.
fn file_highlight(fragments: &Fragments<'_>, text: &str) -> Result<Output, String> {
    let title = highlight::parse(text)
        .map_err(|error| error.to_string())?
        .title;
    let slug =
        entry::derive_slug(&entry::plain_text(&title)).unwrap_or_else(|| "highlight".to_owned());
    let path = entry::unused(&fragments.dir.join(HIGHLIGHTS), &slug, |slug| {
        format!("{slug}.md")
    });

    let planned = Planned {
        shown: fragments.show(&path),
        text: fragments.formatting.apply(&path, text).into_owned(),
        path,
    };
    planned.write()?;

    Ok(format!("Wrote {}\n", planned.shown).into())
}

/// Open the editor on `path`, which messages call `shown`, until what it leaves
/// follows the format `check` holds it to; `opened` is what the file held when
/// it was first opened.
///
/// The error is why the edit was given up on: the editor failed, or what it
/// left doesn't follow the format and the user chose not to edit it again.
fn settle(
    editing: &mut impl Editing,
    path: &Utf8Path,
    shown: &str,
    opened: &str,
    check: &impl Fn(&str) -> Result<(), String>,
) -> Result<Ended, String> {
    loop {
        if !editing.open(path)? {
            return Err("the editor exited with an error".to_owned());
        }

        let text =
            fs::read_to_string(path).map_err(|error| format!("failed to read {shown}: {error}"))?;
        if text.trim().is_empty() {
            return Ok(Ended::Emptied);
        }
        if text == opened {
            return Ok(Ended::Unchanged);
        }

        match check(&text) {
            Ok(()) => return Ok(Ended::Accepted(text)),
            Err(problem) if editing.again(&format!("{shown}: {problem}")) => {}
            Err(problem) => return Err(problem),
        }
    }
}

/// Hold `text` to the format `parse` holds it to, as written and as a write to
/// `path` lays it out: laying a paragraph out a sentence per line can start a
/// line with an upgrade section's token.
fn checked(
    fragments: &Fragments<'_>,
    path: &Utf8Path,
    text: &str,
    parse: &impl Fn(&str) -> Result<(), String>,
) -> Result<(), String> {
    parse(text)?;

    let laid_out = fragments.formatting.apply(path, text);
    if laid_out != text {
        parse(&laid_out)
            .map_err(|problem| format!("once laid out as the workspace formats it, {problem}"))?;
    }

    Ok(())
}

/// Write `text`, which the editor left at `path`, laid out as the workspace
/// formats it, when that changes it.
fn lay_out(
    fragments: &Fragments<'_>,
    path: &Utf8Path,
    shown: &str,
    text: &str,
) -> Result<(), String> {
    let laid_out = fragments.formatting.apply(path, text);
    if laid_out == text {
        return Ok(());
    }

    fs::write(path, laid_out.as_ref()).map_err(|error| format!("failed to write {shown}: {error}"))
}

/// Create the file `path`, which messages call `shown`, holding `text`, and the
/// directory it goes in.
///
/// Refuses to overwrite a file.
fn create(path: &Utf8Path, shown: &str, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|error| format!("failed to create {dir}: {error}"))?;
    }

    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .and_then(|mut file| file.write_all(text.as_bytes()))
        .map_err(|error| format!("failed to write {shown}: {error}"))
}

fn remove(path: &Utf8Path, shown: &str) -> Result<(), String> {
    fs::remove_file(path).map_err(|error| format!("failed to remove {shown}: {error}"))
}

/// `message`, saying also why undoing an edit failed when `undo` did.
fn undone(undo: Result<(), String>, message: String) -> String {
    match undo {
        Ok(()) => message,
        Err(failed) => format!("{message}\n{failed}"),
    }
}

#[cfg(test)]
#[path = "edit_tests.rs"]
mod tests;
