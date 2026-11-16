//! Context the bridge looks up and appends to a message before its turn runs.
//!
//! [`Injector::lookups`] decides what to look up for a message: a command for
//! each id it names that matches an `inject.references` pattern, then the
//! `inject.packs` entry the decide model picked, if its probability meets
//! `inject.threshold`.
//! [`Injector::inject`] runs those commands concurrently in the workspace, and
//! [`render`] appends their output to the message under [`HEADING`].

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

/// The most ids one message has looked up, in the order it names them.
const MAX_REFERENCES: usize = 5;

/// How long one lookup may run before it is abandoned.
const TIMEOUT: Duration = Duration::from_secs(10);

/// A lookup's output, or why it failed.
pub(crate) type Output = Pin<Box<dyn Future<Output = Result<String, String>> + Send>>;

/// Runs a lookup's command.
pub(crate) trait Run: Send + Sync {
    /// Run `command` (program, then arguments) and return its standard output.
    fn run(&self, command: &[String]) -> Output;
}

/// [`Run`] as child processes in the workspace root.
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

/// One command to run for a message, and the heading of its output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Lookup {
    pub title: String,
    pub command: Vec<String>,
}

/// A lookup that produced output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Section {
    pub lookup: Lookup,
    pub output: String,
}

/// What the lookups for one message produced.
#[derive(Debug)]
pub(crate) struct Injected {
    pub sections: Vec<Section>,

    /// Lookups that failed, each with its error.
    pub failures: Vec<(Lookup, String)>,

    pub took: Duration,
}

/// Looks up context for messages, as `inject` configures.
pub(crate) struct Injector {
    options: InjectOptions,
    runner: Arc<dyn Run>,
}

impl Injector {
    pub(crate) fn new(options: InjectOptions, runner: Arc<dyn Run>) -> Self {
        Self { options, runner }
    }

    /// Characters of one lookup's output kept by [`render`].
    pub(crate) fn max_chars(&self) -> usize {
        self.options.max_chars
    }

    /// What to look up for `text`: each distinct reference match, in the order
    /// `text` names them, up to [`MAX_REFERENCES`], then `pick`'s pack when its
    /// probability meets the threshold.
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

    /// Run `lookups` concurrently, keeping their order.
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

/// `command` with `{0}` replaced by the whole match in `captures`, and `{1}`
/// onwards by its groups; a group that didn't match becomes empty.
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

/// `text` with `sections` appended under [`HEADING`], each section's output
/// truncated to `max_chars` characters.
///
/// `today` dates the section, so the assistant reads it as a snapshot on a
/// later turn.
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

/// `output` cut to `max_chars` characters, saying so when it was cut.
fn truncate(output: &str, max_chars: usize) -> String {
    match output.char_indices().nth(max_chars) {
        None => output.to_owned(),
        Some((end, _)) => format!("{}\n[Truncated at {max_chars} characters.]", &output[..end]),
    }
}

/// A code fence longer than any run of backticks in `output`, so a fence inside
/// the output can't close it.
fn fence(output: &str) -> String {
    let longest = output.split(|c| c != '`').map(str::len).max().unwrap_or(0);

    "`".repeat(longest.max(2) + 1)
}

#[cfg(test)]
#[path = "inject_tests.rs"]
mod tests;
