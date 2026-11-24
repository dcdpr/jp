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
//!
//! A run over several conversations asks for each, and keeps an [`Answers`]:
//! `Y` and `N` answer every conversation that remains, and `--yes` is the same
//! answer given before the first question.

use std::borrow::Cow;

use clap::{Arg, ArgAction, ArgMatches, Command};
use crossterm::style::Stylize as _;
use inquire::InquireError;
use jp_conversation::ConversationId;
use jp_inquire::{InlineOption, InlineSelect};
use jp_printer::Printer;
use jp_storage::backend::{Projection, StoredConversation};
use jp_term::sanitize::strip_controls;
use jp_workspace::ConversationLock;

use crate::{
    cli::CommandArgs,
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
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ConfirmFlag {
    /// `--confirm`.
    confirm: bool,

    /// `--no-confirm`, `--yes`, or `-y`.
    no_confirm: bool,
}

impl CommandArgs for ConfirmFlag {
    fn augment(cmd: Command) -> Command {
        cmd.arg(
            Arg::new("confirm")
                .long("confirm")
                .action(ArgAction::SetTrue)
                .overrides_with("no_confirm")
                .help("Prompt for confirmation before each change"),
        )
        .arg(
            Arg::new("no_confirm")
                .short('y')
                .long("no-confirm")
                .visible_alias("yes")
                .action(ArgAction::SetTrue)
                .overrides_with("confirm")
                .help("Skip confirmation prompts"),
        )
    }

    fn from_matches(matches: &ArgMatches) -> std::result::Result<Self, clap::Error> {
        Ok(Self {
            confirm: matches.get_flag("confirm"),
            no_confirm: matches.get_flag("no_confirm"),
        })
    }
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

    /// The answers a run that asks before each change starts with: every
    /// conversation accepted under `--no-confirm`, none settled otherwise.
    pub(crate) fn answers(self) -> Answers {
        if self.preference() == Some(false) {
            Answers::yes_to_all()
        } else {
            Answers::default()
        }
    }
}

/// The user's answer to one confirmation in a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Answer {
    /// `y`: this conversation.
    Yes,

    /// `Y`: this conversation and every one that remains.
    YesToAll,

    /// `n` or Esc: not this conversation.
    No,

    /// `N`: not this conversation, nor any that remains.
    NoToAll,
}

impl Answer {
    const fn accepts(self) -> bool {
        matches!(self, Self::Yes | Self::YesToAll)
    }
}

/// The answers given so far in a run that confirms each conversation.
///
/// Holds the answer `Y` or `N` gave for every remaining conversation, if one
/// was given.
#[derive(Debug, Default)]
pub(crate) struct Answers {
    rest: Option<bool>,
}

impl Answers {
    /// Answers that accept every conversation without asking, as `--yes` does.
    pub(crate) const fn yes_to_all() -> Self {
        Self { rest: Some(true) }
    }

    /// The answer given for every remaining conversation, if one was.
    ///
    /// `Some(false)` means the rest are declined, so a run can stop.
    pub(crate) const fn rest(&self) -> Option<bool> {
        self.rest
    }

    /// Whether the next conversation is accepted: the answer given for every
    /// remaining one, or else the one `ask` returns.
    ///
    /// `more` says whether more conversations follow this one; it is passed to
    /// `ask`, which offers `Y` and `N` only when it is true.
    ///
    /// # Errors
    ///
    /// Returns `ask`'s error.
    pub(crate) fn decide<E>(
        &mut self,
        more: bool,
        ask: impl FnOnce(bool) -> std::result::Result<Answer, E>,
    ) -> std::result::Result<bool, E> {
        if let Some(rest) = self.rest {
            return Ok(rest);
        }

        let answer = ask(more)?;
        match answer {
            Answer::YesToAll => self.rest = Some(true),
            Answer::NoToAll => self.rest = Some(false),
            Answer::Yes | Answer::No => {}
        }
        Ok(answer.accepts())
    }

    /// Whether `action` goes ahead on the locked conversation: the answer given
    /// for every remaining conversation, or else the one
    /// [`confirm_conversation_action`] asks for.
    ///
    /// # Errors
    ///
    /// Fails in the cases [`confirm_conversation_action`] does.
    pub(crate) fn confirm_action(
        &mut self,
        ctx: &Ctx,
        action: ConversationAction,
        lock: &ConversationLock,
        active_id: Option<ConversationId>,
        more: bool,
    ) -> Result<bool> {
        self.decide(more, |more| {
            confirm_conversation_action(ctx, action, lock, active_id, more)
        })
    }
}

/// The wording of a confirmation prompt.
#[derive(Debug, Clone)]
pub(crate) struct Prompt {
    pub question: Cow<'static, str>,

    /// What `y` does.
    pub accept: &'static str,

    /// What `Y` does.
    pub accept_rest: &'static str,

    /// What `n` does.
    pub decline: &'static str,

    /// What `N` does.
    pub decline_rest: &'static str,

    /// Shown beneath the question for as long as it is open.
    pub caution: Option<&'static str>,
}

/// Exit status for a run the user ended with Ctrl-C.
///
/// The shell's convention for a process killed by SIGINT (128 + 2).
/// `inquire` reads Ctrl-C as a keypress in raw mode rather than letting the
/// signal through, so JP reports the status the shell would otherwise have set
/// itself.
pub(crate) const INTERRUPTED_EXIT_STATUS: u8 = 130;

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

    /// A caution shown beneath the question for as long as it is open.
    ///
    /// `None` for an action the user can walk back unaided.
    const fn caution(self) -> Option<&'static str> {
        match self {
            Self::Remove => Some("this action cannot be undone"),
            Self::Archive => None,
        }
    }

    /// The question and the help for each answer.
    pub(crate) const fn prompt(self) -> Prompt {
        match self {
            Self::Remove => Prompt {
                question: Cow::Borrowed("Remove this conversation?"),
                accept: "yes, remove it",
                accept_rest: "yes, and remove every remaining one",
                decline: "no, keep it",
                decline_rest: "no, and keep every remaining one",
                caution: self.caution(),
            },
            Self::Archive => Prompt {
                question: Cow::Borrowed("Archive this conversation?"),
                accept: "yes, archive it",
                accept_rest: "yes, and archive every remaining one",
                decline: "no, leave it where it is",
                decline_rest: "no, and leave every remaining one",
                caution: self.caution(),
            },
        }
    }
}

/// Ask the user to confirm `action` on the locked conversation.
///
/// Prints a heading, the conversation's details, and a single-key question, and
/// returns the user's answer.
/// `n` and Esc both decline, which leaves the conversation untouched and lets a
/// run covering several of them carry on to the next.
/// `more` offers `Y` and `N`, which answer every conversation that remains.
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
    more: bool,
) -> Result<Answer> {
    let id = lock.id();
    ensure_answerable(ctx, action, id)?;

    let pretty = ctx.printer.pretty_printing_enabled();
    let details = action_details(lock, active_id, pretty).with_heading(heading(
        action.progressive(),
        id,
        pretty,
    ));

    ask(&ctx.printer, &action.prompt(), &details.to_string(), more)
}

/// Ask the user to confirm removing a conversation that is not in the
/// conversation list.
///
/// The prompt names the conversation, its title when one is recorded, why it
/// does not load when it does not, and every location the removal deletes.
/// Errors in the same cases as [`confirm_conversation_action`].
pub(crate) fn confirm_unloaded_removal(
    ctx: &Ctx,
    stored: &StoredConversation,
    more: bool,
) -> Result<Answer> {
    let action = ConversationAction::Remove;
    ensure_answerable(ctx, action, stored.id)?;

    let pretty = ctx.printer.pretty_printing_enabled();
    let details = unloaded_details(stored, pretty);
    ask(&ctx.printer, &action.prompt(), &details.to_string(), more)
}

/// Fail when nobody is available to answer the question for `action` on `id`.
fn ensure_answerable(ctx: &Ctx, action: ConversationAction, id: ConversationId) -> Result<()> {
    if ctx.term.interactive {
        return Ok(());
    }

    Err(CmdError::from(format!(
        "{} conversation {id} needs a confirmation and nobody is available to give one; pass \
         --no-confirm to {} it without asking",
        action.progressive().to_lowercase(),
        action.imperative(),
    ))
    .into())
}

/// The line above a prompt's details, e.g. `Removing conversation jp-c…`.
pub(crate) fn heading(verb: &str, id: ConversationId, pretty: bool) -> String {
    let id_label = if pretty {
        id.to_string().bold().yellow().to_string()
    } else {
        id.to_string()
    };

    format!("{verb} conversation {id_label}")
}

/// Show `details` and ask `prompt`'s single-key question.
///
/// `more` offers `Y` and `N`, which answer every conversation that remains.
pub(crate) fn ask(printer: &Printer, prompt: &Prompt, details: &str, more: bool) -> Result<Answer> {
    // `InlineSelect` splits at the last newline, printing everything before it
    // as a preamble and handing inquire the single line it can redraw.
    let message = format!("{details}\n\n{}", prompt.question);

    let mut select = InlineSelect::new(&message, options(prompt, more)).with_default('n');
    if let Some(caution) = prompt.caution {
        select = select.with_help_message(caution);
    }

    decide(select.prompt(&mut printer.prompt_writer()))
}

/// The keys a prompt offers: `Y` and `N` only when more conversations follow.
fn options(prompt: &Prompt, more: bool) -> Vec<InlineOption> {
    let mut options = vec![InlineOption::new('y', prompt.accept)];
    if more {
        options.push(InlineOption::new('Y', prompt.accept_rest));
    }
    options.push(InlineOption::new('n', prompt.decline));
    if more {
        options.push(InlineOption::new('N', prompt.decline_rest));
    }
    options
}

/// Turn a prompt's result into an answer.
///
/// Esc declines, the same as `n`: the user backed out of the question, which is
/// an answer to it.
/// Ctrl-C is not, and ends the run instead.
/// `inquire` reads it as a keypress in raw mode, so no signal is raised and
/// nothing else would stop a run part-way through a list.
/// Every other error keeps its cause, so a broken terminal is reported as a
/// failure rather than as a choice.
fn decide(answer: std::result::Result<char, InquireError>) -> Result<Answer> {
    match answer {
        Ok('y') => Ok(Answer::Yes),
        Ok('Y') => Ok(Answer::YesToAll),
        Ok('N') => Ok(Answer::NoToAll),
        Ok(_) | Err(InquireError::OperationCanceled) => Ok(Answer::No),
        Err(InquireError::OperationInterrupted) => {
            Err(CmdError::from(INTERRUPTED_EXIT_STATUS).into())
        }
        Err(error) => Err(error.into()),
    }
}

/// Build the details block shown above the prompt for removing a conversation
/// that is not in the conversation list.
fn unloaded_details(stored: &StoredConversation, pretty: bool) -> DetailsFmt {
    stored_details(stored, pretty).with_heading(heading(
        ConversationAction::Remove.progressive(),
        stored.id,
        pretty,
    ))
}

/// Build the details of a conversation read from the store without loading it:
/// its title when one is recorded, why it does not load, and its locations.
///
/// The block is a prompt's preamble, which the prompt widget writes itself, so
/// a title keeps no control character there.
pub(crate) fn stored_details(stored: &StoredConversation, pretty: bool) -> DetailsFmt {
    DetailsFmt::new(stored.id)
        .with_title(
            stored
                .title
                .as_deref()
                .map(|title| strip_controls(title, &[])),
        )
        .with_status(stored.state.clone().err())
        .with_locations(stored.locations.clone())
        .with_pretty_printing(pretty)
}

/// Build the details block shown above a confirmation prompt.
///
/// The block is the prompt's preamble, which the prompt widget writes itself,
/// so a title keeps no control character there, whatever `style.sanitize` says.
fn action_details(
    lock: &ConversationLock,
    active_id: Option<ConversationId>,
    pretty: bool,
) -> DetailsFmt {
    let id = lock.id();
    let meta = lock.metadata();
    let events = lock.events();

    DetailsFmt::new(id)
        .with_title(
            meta.title
                .as_deref()
                .map(|title| strip_controls(title, &[])),
        )
        .with_event_count(events.len())
        .with_turn_count(events.turn_count())
        .with_last_message_at(events.last_turn_event().map(|event| event.timestamp))
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
