//! Pre-turn context lookups appended to a message before its turn runs.
//!
//! [`Injector::lookups`] collects the commands to run for a message: one for
//! each distinct match of an `inject.references` pattern (up to
//! [`MAX_REFERENCES`]), followed by the `inject.packs` entry selected by the
//! decide model when its probability meets `inject.threshold`.
//! [`Injector::inject`] runs those commands concurrently in the workspace root,
//! and [`render`] appends their output to the message under [`HEADING`].

use std::{
    collections::BTreeSet, future::Future, pin::Pin, process::Stdio, sync::Arc, time::Duration,
};

use camino::Utf8PathBuf;
use chrono::NaiveDate;
use regex::Captures;
use tokio::{
    process::Command,
    task::JoinSet,
    time::{Instant, timeout},
};

use crate::{decide::Pick, options::InjectOptions};

/// Heading of the section appended to a message.
pub(crate) const HEADING: &str = "# Auto-Injected Context";

/// Maximum number of reference lookups per message, in match order.
const MAX_REFERENCES: usize = 5;

/// Per-command execution timeout before the child process is killed.
const TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) type Output = Pin<Box<dyn Future<Output = Result<String, String>> + Send>>;

pub(crate) trait Run: Send + Sync {
    /// Run `command` (program and arguments) and return its standard output.
    fn run(&self, command: &[String]) -> Output;
}

/// [`Run`] implementation that spawns child processes in the workspace root.
pub(crate) struct Process {
    root: Utf8PathBuf,
}

impl Process {
    pub(crate) fn new(root: Utf8PathBuf) -> Self {
        Self { root }
    }
}

impl Run for Process {
    fn run(&self, command: &[String]) -> Output {
        let program = command.first().cloned().unwrap_or_default();
        let mut child = Command::new(&program);
        child
            .args(command.iter().skip(1))
            .current_dir(&self.root)
            .stdin(Stdio::null())
            .kill_on_drop(true);

        Box::pin(async move {
            let output = timeout(TIMEOUT, child.output())
                .await
                .map_err(|_| format!("`{program}` ran longer than {}s", TIMEOUT.as_secs()))?
                .map_err(|error| format!("failed to run `{program}`: {error}"))?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(format!(
                    "`{program}` exited with {}: {}",
                    output.status,
                    stderr.trim()
                ));
            }

            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        })
    }
}

/// A context lookup command and the markdown heading for its output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Lookup {
    pub title: String,
    pub command: Vec<String>,
}

/// Completed lookup and its captured standard output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Section {
    pub lookup: Lookup,
    pub output: String,
}

/// Results of running all context lookups for a single message.
#[derive(Debug)]
pub(crate) struct Injected {
    pub sections: Vec<Section>,
    pub failures: Vec<(Lookup, String)>,
    pub took: Duration,
}

/// Builds and runs pre-turn context lookups configured under `inject`.
pub(crate) struct Injector {
    options: InjectOptions,
    runner: Arc<dyn Run>,
}

impl Injector {
    pub(crate) fn new(options: InjectOptions, runner: Arc<dyn Run>) -> Self {
        Self { options, runner }
    }

    /// Maximum characters retained per lookup output by [`render`].
    pub(crate) fn max_chars(&self) -> usize {
        self.options.max_chars
    }

    /// Collect the lookups for `text`: up to [`MAX_REFERENCES`] distinct
    /// reference matches in order of appearance, followed by the pack in `pick`
    /// if its probability meets `options.threshold`.
    pub(crate) fn lookups(&self, text: &str, pick: Option<&Pick>) -> Vec<Lookup> {
        let mut seen = BTreeSet::new();
        let mut lookups = vec![];
        for reference in &self.options.references {
            for captures in reference.pattern.captures_iter(text) {
                let Some(whole) = captures.get(0) else {
                    continue;
                };
                if lookups.len() == MAX_REFERENCES || !seen.insert(whole.as_str().to_owned()) {
                    continue;
                }

                lookups.push(Lookup {
                    title: whole.as_str().to_owned(),
                    command: substitute(&reference.command, &captures),
                });
            }
        }

        let pack = pick
            .filter(|pick| pick.probability >= self.options.threshold)
            .and_then(|pick| self.options.packs.get(&pick.name));
        lookups.extend(pack.map(|pack| Lookup {
            title: pack.title.clone(),
            command: pack.command.clone(),
        }));

        lookups
    }

    /// Run `lookups` concurrently and return results in input order.
    pub(crate) async fn inject(&self, lookups: Vec<Lookup>) -> Injected {
        let started = Instant::now();
        let mut running = JoinSet::new();
        for (index, lookup) in lookups.iter().enumerate() {
            let output = self.runner.run(&lookup.command);
            running.spawn(async move { (index, output.await) });
        }

        let mut results: Vec<(usize, Result<String, String>)> =
            running.join_all().await.into_iter().collect();
        results.sort_by_key(|(index, _)| *index);

        let mut injected = Injected {
            sections: vec![],
            failures: vec![],
            took: started.elapsed(),
        };
        for ((_, result), lookup) in results.into_iter().zip(lookups) {
            match result {
                Ok(output) => injected.sections.push(Section { lookup, output }),
                Err(error) => injected.failures.push((lookup, error)),
            }
        }

        injected
    }
}

/// Replace `{0}` in each argument of `command` with the full regex match and
/// `{1}`, `{2}`, ... with capture groups (or `""` for unmatched optional
/// groups).
fn substitute(command: &[String], captures: &Captures<'_>) -> Vec<String> {
    command
        .iter()
        .map(|arg| {
            (0..captures.len()).fold(arg.clone(), |arg, group| {
                let value = captures.get(group).map_or("", |m| m.as_str());
                arg.replace(&format!("{{{group}}}"), value)
            })
        })
        .collect()
}

/// Append `sections` to `text` under [`HEADING`], truncating each section's
/// output to `max_chars` characters.
///
/// Includes `today` in the section header so later turns treat the output as a
/// snapshot from that date.
pub(crate) fn render(
    text: &str,
    today: NaiveDate,
    sections: &[Section],
    max_chars: usize,
) -> String {
    let mut out = format!(
        "{text}\n\n{HEADING}\n\nThe bridge looked this up on {today} because the message above \
         seemed to need it. The sender didn't write it, and it may be incomplete or irrelevant."
    );
    for section in sections {
        let output = truncate(section.output.trim_end(), max_chars);
        let fence = fence(&output);
        out.push_str(&format!(
            "\n\n## {}\n\nOutput of `{}`:\n\n{fence}text\n{output}\n{fence}",
            section.lookup.title,
            section.lookup.command.join(" ")
        ));
    }

    out
}

/// Truncate `output` to `max_chars` characters, appending a truncation notice
/// when shortened.
fn truncate(output: &str, max_chars: usize) -> String {
    match output.char_indices().nth(max_chars) {
        None => output.to_owned(),
        Some((end, _)) => format!("{}\n[Truncated at {max_chars} characters.]", &output[..end]),
    }
}

/// Return a backtick code fence longer than any run of backticks in `output`
/// (minimum 3) so embedded fences cannot close the block early.
fn fence(output: &str) -> String {
    let longest = output.split(|c| c != '`').map(str::len).max().unwrap_or(0);

    "`".repeat(longest.max(2) + 1)
}

#[cfg(test)]
#[path = "inject_tests.rs"]
mod tests;
