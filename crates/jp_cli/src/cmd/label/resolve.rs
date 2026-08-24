//! Resolution of configured label rules into concrete label values.
//!
//! A rule produces the values of exactly one key: literals taken as they are
//! written, or one value per line of a command's stdout.
//! Command execution is gated by the rule's `run` policy, which may prompt.
//!
//! Two entry points with deliberately different failure semantics:
//!
//! - [`Resolver::automatic`] applies every rule matching a [`Trigger`].
//!   A rule that can't be resolved is dropped with a warning and the
//!   surrounding command still succeeds, because the user didn't ask for that
//!   specific label.
//! - [`Resolver::alias`] resolves one rule the user named on the command line.
//!   Failures are errors, because silently omitting a label the user asked for
//!   would be dishonest.
//!   Declining the confirmation prompt is the exception: the user has just said
//!   no, so the label is dropped and the command continues.
//!
//! Declining and cancelling are different answers.
//! An explicit `n` drops the one label; a prompt error (Ctrl-C, Esc, a closed
//! terminal) aborts the surrounding command, because the user asked to stop.

use camino::Utf8Path;
use indexmap::IndexMap;
use jp_config::{
    conversation::label::{LabelConfig, LabelRunMode, LabelValueRef},
    types::command::{CommandConfig, shell_command_line},
};
use jp_inquire::{InlineOption, prompt::PromptBackend};
use jp_printer::Printer;
use tokio::process::Command;
use tracing::warn;

use crate::error::{Error, Result};

/// The event a rule is being applied for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Trigger {
    /// A new conversation is being created (`apply_on.new`).
    New,

    /// An existing conversation is being forked (`apply_on.fork`).
    Fork,
}

/// Resolves `conversation.labels` rules against the filesystem and the user.
pub(crate) struct Resolver<'a> {
    rules: &'a IndexMap<String, LabelConfig>,
    root: &'a Utf8Path,
    is_tty: bool,
    printer: &'a Printer,
    prompts: &'a dyn PromptBackend,
}

impl<'a> Resolver<'a> {
    pub(crate) const fn new(
        rules: &'a IndexMap<String, LabelConfig>,
        root: &'a Utf8Path,
        is_tty: bool,
        printer: &'a Printer,
        prompts: &'a dyn PromptBackend,
    ) -> Self {
        Self {
            rules,
            root,
            is_tty,
            printer,
            prompts,
        }
    }

    /// Resolve every rule that opts into `trigger`, in declaration order.
    ///
    /// A rule that resolves to no values keeps its key, with no values against
    /// it: the rule matched and produced nothing, which is what lets a caller
    /// replace the key's set with nothing.
    /// A rule that fails is dropped entirely, so its key is absent and whatever
    /// the conversation carries is left alone.
    ///
    /// # Errors
    ///
    /// Returns an error when a rule needs confirmation and there is no terminal
    /// to ask on, or when the confirmation prompt is cancelled or the prompt
    /// backend fails.
    /// Every other failure drops that one label and leaves the rest intact.
    pub(crate) async fn automatic(
        &self,
        trigger: Trigger,
    ) -> Result<IndexMap<String, Vec<String>>> {
        let wanted: Vec<_> = self
            .rules
            .iter()
            .filter(|(_, rule)| match trigger {
                Trigger::New => rule.apply_on().new,
                Trigger::Fork => rule.apply_on().fork,
            })
            .collect();

        let mut resolved: IndexMap<String, Vec<String>> = IndexMap::new();
        let mut pending = Vec::new();

        // Prompting is interactive and therefore serial; the commands the user
        // approves are then run concurrently below.
        for (key, rule) in wanted {
            match rule.value() {
                LabelValueRef::Values(values) => {
                    resolved.insert(key.clone(), values.to_vec());
                }
                LabelValueRef::Command(cmd) => {
                    let cmd = cmd.clone().command();
                    match self.approve(key, &cmd, rule.run())? {
                        Approval::Approved => pending.push((key.clone(), cmd)),
                        Approval::Declined => {}
                    }
                }
            }
        }

        for (key, output) in run_all(pending, self.root).await {
            match output {
                Ok(values) => {
                    resolved.insert(key, values);
                }
                Err(error) => {
                    self.report_skipped(&key, &error);
                }
            }
        }

        Ok(resolved)
    }

    /// Resolve the rule named `key`, ignoring its `apply_on` policy.
    ///
    /// Returns the values the rule produces, which may be none.
    /// Returns `Ok(None)` when the user declines the confirmation prompt.
    ///
    /// # Errors
    ///
    /// Returns an error when no rule is configured under `key`, when the rule
    /// is `run = "deny"`, when there is no terminal to confirm on, or when the
    /// command fails.
    pub(crate) async fn alias(&self, key: &str) -> Result<Option<(String, Vec<String>)>> {
        let rule = self.rules.get(key).ok_or_else(|| {
            Error::Label(format!(
                "unknown label alias ':{key}': no `conversation.labels.{key}` is configured"
            ))
        })?;

        let cmd = match rule.value() {
            LabelValueRef::Values(values) => {
                return Ok(Some((key.to_owned(), values.to_vec())));
            }
            LabelValueRef::Command(cmd) => cmd.clone().command(),
        };

        // `deny` is a refusal to run this command at all. Under automatic
        // application that silently drops the label; asked for by name it has
        // to be reported, or the user is left wondering where it went.
        if rule.run() == LabelRunMode::Deny {
            return Err(Error::Label(format!(
                "label ':{key}' is configured with `run = \"deny\"`, so its command is never run"
            )));
        }

        match self.approve(key, &cmd, rule.run())? {
            Approval::Declined => {
                self.printer
                    .eprintln(format!("⚠ Skipping label '{key}': command not run."));
                Ok(None)
            }
            Approval::Approved => run_command(&cmd, self.root)
                .await
                .map(|values| Some((key.to_owned(), values)))
                .map_err(|error| Error::Label(format!("label ':{key}' failed: {error}"))),
        }
    }

    /// Decide whether `cmd` may run under `run`, prompting when required.
    ///
    /// Only an explicit `n` declines; every other answer to the prompt is
    /// treated as a decline too, but a *failure* to prompt is not an answer.
    ///
    /// # Errors
    ///
    /// Returns an error when confirmation is required and no terminal is
    /// available, since neither running nor skipping is a safe assumption, and
    /// when the prompt is cancelled (Ctrl-C, Esc) or the backend fails.
    fn approve(&self, key: &str, cmd: &CommandConfig, run: LabelRunMode) -> Result<Approval> {
        match run {
            LabelRunMode::Unattended => Ok(Approval::Approved),
            LabelRunMode::Deny => Ok(Approval::Declined),
            LabelRunMode::Ask if !self.is_tty => Err(Error::Label(format!(
                "label '{key}' needs confirmation to run `{cmd}`, but there is no terminal to ask \
                 on; set `conversation.labels.{key}.run` to \"unattended\" or \"deny\""
            ))),
            LabelRunMode::Ask => {
                let question = format!("Run `{cmd}` for label '{key}'?");
                let options = vec![
                    InlineOption::new('y', "Run it"),
                    InlineOption::new('n', "Skip this label"),
                ];
                let mut writer = self.printer.prompt_writer();

                // Only an explicit `n` declines. A prompt error is a
                // cancellation, not an answer, so it aborts the surrounding
                // command rather than quietly dropping the label and carrying
                // on to create a conversation and send a turn.
                match self
                    .prompts
                    .inline_select(&question, options, Some('n'), &mut writer)?
                {
                    'y' => Ok(Approval::Approved),
                    _ => Ok(Approval::Declined),
                }
            }
        }
    }

    /// Report a label that was dropped rather than applied.
    ///
    /// The detail goes to the diagnostics channel; a one-line notice goes to
    /// the chrome channel so a failing label rule isn't invisible.
    fn report_skipped(&self, key: &str, error: &str) {
        warn!(label = key, %error, "Skipping label.");
        self.printer
            .eprintln(format!("⚠ Skipping label '{key}': {error}"));
    }
}

/// Whether a label's command may run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Approval {
    Approved,
    Declined,
}

/// Run every approved command concurrently, pairing each result with its key.
async fn run_all(
    pending: Vec<(String, CommandConfig)>,
    root: &Utf8Path,
) -> Vec<(String, std::result::Result<Vec<String>, String>)> {
    let futures = pending.into_iter().map(|(key, cmd)| async move {
        let result = run_command(&cmd, root).await;
        (key, result)
    });

    futures::future::join_all(futures).await
}

/// Run a label command at `root` and return one value per line of its stdout.
///
/// Empty lines are dropped, so a trailing newline does not add a value and a
/// command that writes nothing produces none.
/// Splitting on lines rather than a delimiter is what lets a value contain any
/// character without an escaping rule.
///
/// A `shell = true` command is handed to `sh -c` with its arguments quoted, so
/// pipes and `&&` work; otherwise the program is executed directly.
/// A non-zero exit is an error, and stderr is folded into the message so the
/// reason is visible without re-running by hand.
///
/// The child stays in JP's process group, so Ctrl-C reaches it.
/// Tool commands deliberately detach (`process_group(0)`) because JP drives
/// their lifecycle through a cancellation token; a label command has no such
/// handle, and blocking conversation creation on an unkillable command would be
/// worse than losing the label.
async fn run_command(
    cmd: &CommandConfig,
    root: &Utf8Path,
) -> std::result::Result<Vec<String>, String> {
    let mut command = if cmd.shell {
        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(shell_command_line(&cmd.program, &cmd.args));
        command
    } else {
        let mut command = Command::new(&cmd.program);
        command.args(&cmd.args);
        command
    };

    let output = command
        .current_dir(root.as_std_path())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|error| format!("could not run `{cmd}`: {error}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        let code = output
            .status
            .code()
            .map_or_else(|| "signal".to_owned(), |c| c.to_string());

        return Err(if detail.is_empty() {
            format!("`{cmd}` exited with status {code}")
        } else {
            format!("`{cmd}` exited with status {code}: {detail}")
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

#[cfg(test)]
#[path = "resolve_tests.rs"]
mod tests;
