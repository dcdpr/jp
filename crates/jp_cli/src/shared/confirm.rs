//! Shared confirmation for mutating conversation commands.
//!
//! [`ConfirmFlag`] exposes `--confirm`, `--no-confirm`, and the `--yes` / `-y`
//! alias.
//! With no flag, the decision is left to the command's own default.
//!
//! [`confirm_conversation_action`] is the prompt itself: a heading naming the
//! action, the conversation's details, and a single-key question.
//! It reads the conversation through a held [`ConversationLock`], so the
//! details describe the state the answer acts on and no other process can
//! change that state while the question is open.

use crossterm::style::Stylize as _;
use inquire::InquireError;
use jp_conversation::ConversationId;
use jp_inquire::{InlineOption, InlineSelect};
use jp_storage::backend::Projection;
use jp_workspace::ConversationLock;

use crate::{
    cmd::Error as CmdError,
    ctx::Ctx,
    error::Result,
    format::{conversation::DetailsFmt, label_detail_items},
};

/// Confirmation-prompt preference shared by mutating commands.
///
/// `--confirm` forces a prompt before each change; `--no-confirm` (alias
/// `--yes`, `-y`) skips it.
/// With neither flag, [`Self::preference`] returns `None` and the command
/// applies its own default.
#[derive(Debug, Clone, Copy, Default, clap::Args)]
pub(crate) struct ConfirmFlag {
    /// Prompt for confirmation before each change.
    #[arg(long, overrides_with = "no_confirm")]
    confirm: bool,

    /// Skip confirmation prompts.
    #[arg(
        long = "no-confirm",
        visible_alias = "yes",
        short = 'y',
        overrides_with = "confirm"
    )]
    no_confirm: bool,
}

impl ConfirmFlag {
    /// The user's explicit preference, or `None` when no confirm flag was
    /// passed.
    ///
    /// `Some(true)` always prompts, `Some(false)` never prompts, and `None`
    /// defers to the command's default.
    /// When both flags appear, the last one on the command line wins.
    pub(crate) fn preference(self) -> Option<bool> {
        if self.confirm {
            Some(true)
        } else if self.no_confirm {
            Some(false)
        } else {
            None
        }
    }
}

/// Exit status for a run the user ended with Ctrl-C.
///
/// The shell's convention for a process killed by SIGINT (128 + 2).
/// `inquire` reads Ctrl-C as a keypress in raw mode rather than letting the
/// signal through, so JP reports the status the shell would otherwise have set
/// itself.
const INTERRUPTED_EXIT_STATUS: u8 = 130;

/// A conversation-mutating action that asks before it proceeds.
///
/// Carries the wording around the prompt, so a caller picks a verb rather than
/// a phrasing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConversationAction {
    /// Permanent removal.
    Remove,

    /// Archival, reversible with `jp c unarchive`.
    Archive,
}

impl ConversationAction {
    /// The verb in the heading, e.g. `Removing`.
    const fn progressive(self) -> &'static str {
        match self {
            Self::Remove => "Removing",
            Self::Archive => "Archiving",
        }
    }

    /// The bare verb, e.g. `remove`.
    const fn imperative(self) -> &'static str {
        match self {
            Self::Remove => "remove",
            Self::Archive => "archive",
        }
    }

    /// The question the user answers.
    const fn question(self) -> &'static str {
        match self {
            Self::Remove => "Remove this conversation?",
            Self::Archive => "Archive this conversation?",
        }
    }

    /// A caution shown beneath the question for as long as it is open.
    ///
    /// `None` for an action the user can walk back unaided.
    const fn caution(self) -> Option<&'static str> {
        match self {
            Self::Remove => Some("this action cannot be undone"),
            Self::Archive => None,
        }
    }

    /// What accepting does, listed under the prompt's `?` help.
    const fn accept_help(self) -> &'static str {
        match self {
            Self::Remove => "yes, remove it",
            Self::Archive => "yes, archive it",
        }
    }

    /// What declining does, listed under the prompt's `?` help.
    const fn decline_help(self) -> &'static str {
        match self {
            Self::Remove => "no, keep it",
            Self::Archive => "no, leave it where it is",
        }
    }
}

/// Ask the user to confirm `action` on the locked conversation.
///
/// Prints a heading, the conversation's details, and a single-key question, and
/// returns whether the user accepted.
/// `n` and Esc both decline, which leaves the conversation untouched and lets a
/// run covering several of them carry on to the next.
/// `active_id` is the session's active conversation, which the details mark as
/// such.
///
/// Errors rather than declining when nobody is available to answer, when the
/// user interrupts with Ctrl-C, and when the prompt itself breaks.
/// Reading any of those as a decline would let a bulk run walk past every
/// conversation in it and report success.
pub(crate) fn confirm_conversation_action(
    ctx: &Ctx,
    action: ConversationAction,
    lock: &ConversationLock,
    active_id: Option<ConversationId>,
) -> Result<bool> {
    let id = lock.id();

    if !ctx.term.interactive {
        return Err(CmdError::from(format!(
            "{} conversation {id} needs a confirmation and nobody is available to give one; pass \
             --no-confirm to {} it without asking",
            action.progressive().to_lowercase(),
            action.imperative(),
        ))
        .into());
    }

    let pretty = ctx.printer.pretty_printing_enabled();
    let id_label = if pretty {
        id.to_string().bold().yellow().to_string()
    } else {
        id.to_string()
    };

    let details = action_details(lock, active_id, pretty)
        .with_heading(format!("{} conversation {id_label}", action.progressive()));

    // `InlineSelect` splits at the last newline, printing everything before it
    // as a preamble and handing inquire the single line it can redraw.
    let message = format!("{details}\n\n{}", action.question());
    let options = vec![
        InlineOption::new('y', action.accept_help()),
        InlineOption::new('n', action.decline_help()),
    ];

    let mut select = InlineSelect::new(&message, options).with_default('n');
    if let Some(caution) = action.caution() {
        select = select.with_help_message(caution);
    }

    decide(select.prompt(&mut ctx.printer.prompt_writer()))
}

/// Turn a prompt's result into a decision.
///
/// Esc declines, the same as `n`: the user backed out of the question, which is
/// an answer to it.
/// Ctrl-C is not, and ends the run instead.
/// `inquire` reads it as a keypress in raw mode, so no signal is raised and
/// nothing else would stop a run part-way through a list.
/// Every other error keeps its cause, so a broken terminal is reported as a
/// failure rather than as a choice.
fn decide(answer: std::result::Result<char, InquireError>) -> Result<bool> {
    match answer {
        Ok('y') => Ok(true),
        Ok(_) | Err(InquireError::OperationCanceled) => Ok(false),
        Err(InquireError::OperationInterrupted) => {
            Err(CmdError::from(INTERRUPTED_EXIT_STATUS).into())
        }
        Err(error) => Err(error.into()),
    }
}

/// Build the details block shown above a confirmation prompt.
fn action_details(
    lock: &ConversationLock,
    active_id: Option<ConversationId>,
    pretty: bool,
) -> DetailsFmt {
    let id = lock.id();
    let meta = lock.metadata();
    let events = lock.events();

    DetailsFmt::new(id)
        .with_title(meta.title.as_ref())
        .with_event_count(events.len())
        .with_turn_count(events.iter_turns().len())
        .with_last_message_at(events.last().map(|v| v.event.timestamp))
        .with_last_activated_at(Some(meta.last_activated_at))
        .with_pinned_flag(meta.is_pinned())
        .with_local_flag(matches!(lock.projection(), Projection::LocalOnly))
        .with_labels(label_detail_items(&meta.labels))
        .with_active_conversation(active_id)
        .with_pretty_printing(pretty)
}

#[cfg(test)]
#[path = "confirm_tests.rs"]
mod tests;
