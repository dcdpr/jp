//! Interactive editor workflow for `jp release <type>` and `jp release
//! highlight`.
//!
//! [`fragment()`] and [`highlight()`] open files in the user's editor via
//! [`Editing`] (implemented by [`Host`] over the plugin protocol) and validate
//! the result when the editor exits:
//!
//! - If validation fails, the user is prompted to either reopen the editor or
//!   discard the changes (deleting a newly created file or restoring the
//!   original contents).
//! - Saving an empty file deletes the fragment or cancels creating a new file.
//! - Accepted files are formatted according to the workspace's
//!   `format_on_write` setting.
//!
//! Unnamed highlights are drafted in a temporary file outside the fragments
//! directory and moved to `highlights/<slug>.md` once validated, using a slug
//! derived from the `# Title` heading.

use std::{
    fs,
    io::{self, BufRead, Write},
};

use camino::{Utf8Path, Utf8PathBuf};
use changelog::{Type, fragment, highlight};
use jp_plugin::{
    client,
    message::{
        ComposeMode, ComposeOption, ComposeRequest, EditFilesRequest, HostToPlugin, PluginToHost,
    },
};

use crate::{
    BINARY, Output,
    entry::{self, Fragments, HIGHLIGHTS, Planned},
    repository::temporary,
};

/// Minimum host protocol version that supports `edit_files`.
pub(crate) const EDIT_FILES_PROTOCOL: u32 = 13;

/// Editor interaction interface abstracted for testing.
pub(crate) trait Editing {
    /// Open `path` in the user's editor and return `Ok(true)` if the editor
    /// exited cleanly.
    fn open(&mut self, path: &Utf8Path) -> Result<bool, String>;

    /// Prompt the user after validation fails with `problem`, returning `true`
    /// to reopen the editor or `false` to discard the edit.
    fn again(&mut self, problem: &str) -> bool;
}

/// [`Editing`] implementation backed by the host plugin protocol (`edit_files`
/// and `compose`).
pub(crate) struct Host<'a, R, W> {
    pub(crate) stdin: &'a mut R,
    pub(crate) stdout: &'a mut W,
}

impl<R: BufRead, W: Write> Editing for Host<'_, R, W> {
    fn open(&mut self, path: &Utf8Path) -> Result<bool, String> {
        client::send(
            self.stdout,
            &PluginToHost::EditFiles(EditFilesRequest {
                id: None,
                paths: vec![path.to_owned()],
            }),
        )?;

        match client::read(self.stdin, BINARY)? {
            HostToPlugin::Edited(response) => Ok(response.saved),
            HostToPlugin::Error(error) => {
                Err(format!("couldn't open an editor: {}", error.message))
            }
            HostToPlugin::Shutdown => Err("interrupted".to_owned()),
            other => Err(format!("expected an edited response, got: {other:?}")),
        }
    }

    fn again(&mut self, problem: &str) -> bool {
        let choice = client::compose(self.stdin, self.stdout, BINARY, ComposeRequest {
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

        choice.is_ok_and(|choice| choice == "edit")
    }
}

/// Return an error if the host protocol `version` does not support
/// `edit_files`.
pub(crate) fn check_host(version: u32) -> Result<(), String> {
    if version >= EDIT_FILES_PROTOCOL {
        return Ok(());
    }

    Err(format!(
        "Opening an editor needs `jp` protocol {EDIT_FILES_PROTOCOL}, and this `jp` speaks \
         {version}; reinstall the two together."
    ))
}

/// Open `<slug>.<type>.md` in the editor (creating an empty file if needed) and
/// validate the resulting fragment.
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

/// Open `highlights/<slug>.md` in the editor, or draft a new highlight (seeded
/// with `author`) and file it under the slug derived from its `# Title`.
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

enum Ended {
    Accepted(String),
    Unchanged,
    Emptied,
}

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

/// Edit a new highlight in a temporary file outside the fragments directory,
/// then file it under the slug derived from its `# Title` once validated.
fn drafted(
    fragments: &Fragments<'_>,
    template: &str,
    parse: impl Fn(&str) -> Result<(), String>,
    editing: &mut impl Editing,
) -> Result<Output, String> {
    const SHOWN: &str = "The new highlight";

    let draft = Utf8PathBuf::from(format!("{}.md", temporary("highlight")?));
    create(&draft, SHOWN, template)?;

    let filed_as = fragments.dir.join(HIGHLIGHTS).join("new.md");
    let check = |text: &str| checked(fragments, &filed_as, text, &parse);
    let outcome = match settle(editing, &draft, SHOWN, template, &check) {
        Ok(Ended::Accepted(text)) => file_highlight(fragments, &text),
        Ok(Ended::Unchanged | Ended::Emptied) => Ok("Nothing written.\n".to_owned().into()),
        Err(problem) => Err(format!("Nothing written: {problem}")),
    };

    drop(fs::remove_file(&draft));

    outcome
}

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

/// Repeatedly open `path` in `editing` until `check` succeeds or the user
/// discards the edit.
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

/// Validate `text` with `parse` both as written and after applying workspace
/// formatting.
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

fn undone(undo: Result<(), String>, message: String) -> String {
    match undo {
        Ok(()) => message,
        Err(failed) => format!("{message}\n{failed}"),
    }
}

#[cfg(test)]
#[path = "edit_tests.rs"]
mod tests;
