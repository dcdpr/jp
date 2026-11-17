//! The merge-queue label: which one the repository uses, and adding it to or
//! removing it from a pull request.

use std::{
    sync::mpsc::{self, Sender},
    thread,
};

use camino::{Utf8Path, Utf8PathBuf};

use super::{Error, gh, git, stdout};

/// Default merge-queue label when `dash.queueLabel` is unset in git config.
pub const DEFAULT_QUEUE_LABEL: &str = "merge-when-green";

const QUEUE_LABEL_KEY: &str = "dash.queueLabel";

/// Return the merge-queue label for the repository containing `dir`
/// (`dash.queueLabel` in git config, or [`DEFAULT_QUEUE_LABEL`]).
#[must_use]
pub fn queue_label(dir: &Utf8Path) -> String {
    git::config(dir, QUEUE_LABEL_KEY).unwrap_or_else(|| DEFAULT_QUEUE_LABEL.to_owned())
}

/// Add (`add: true`) or remove (`add: false`) the merge-queue label on pull
/// request `number`, creating the label in the repository first if needed.
pub fn set_queued(dir: &Utf8Path, number: u64, add: bool) -> Result<(), Error> {
    let label = queue_label(dir);
    let edit = || {
        let mut command = gh(dir);
        command
            .args(["pr", "edit", &number.to_string()])
            .arg(if add { "--add-label" } else { "--remove-label" })
            .arg(&label)
            .env("GH_PROMPT_DISABLED", "1");
        stdout("gh", git::noninteractive(&mut command, dir)).map(drop)
    };

    match edit() {
        Err(_) if add => {
            // `gh pr edit --add-label` fails if the label does not exist in the
            // repository yet; create it (ignoring errors if it already exists)
            // and retry so the retry surfaces any underlying error.
            let mut create = gh(dir);
            create
                .args([
                    "label",
                    "create",
                    &label,
                    "--color",
                    "0e8a16",
                    "--description",
                ])
                .arg("Merged by dash once its checks pass")
                .env("GH_PROMPT_DISABLED", "1");
            let _exists = stdout("gh", git::noninteractive(&mut create, dir));
            edit()
        }
        result => result,
    }
}

/// Spawn a background worker that applies `(pr_number, add)` queue-label edits
/// sequentially and invokes `done` with each outcome.
pub fn label_editor<D>(dir: Utf8PathBuf, done: D) -> Sender<(u64, bool)>
where
    D: Fn(u64, bool, Result<(), String>) + Send + 'static,
{
    editor(
        move |number, add| set_queued(&dir, number, add).map_err(|error| error.to_string()),
        done,
    )
}

fn editor<E, D>(edit: E, done: D) -> Sender<(u64, bool)>
where
    E: Fn(u64, bool) -> Result<(), String> + Send + 'static,
    D: Fn(u64, bool, Result<(), String>) + Send + 'static,
{
    let (send, edits) = mpsc::channel();

    // Process label edits on a single thread so rapid toggles (`m` twice)
    // execute in order instead of racing on GitHub.
    thread::spawn(move || {
        for (number, add) in edits {
            done(number, add, edit(number, add));
        }
    });

    send
}

#[cfg(test)]
#[path = "label_tests.rs"]
mod tests;
