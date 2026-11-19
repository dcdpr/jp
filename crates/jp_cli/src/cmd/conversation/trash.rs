//! `jp conversation trash` and `jp conversation untrash`.
//!
//! Both take conversation IDs as typed, without looking them up in the
//! conversation index, so they work on conversations that fail to load and on
//! unstored query drafts, neither of which the index lists.

use clap::{ArgMatches, Command};
use crossterm::style::Stylize as _;
use jp_conversation::ConversationId;
use jp_storage::{
    backend::FsStorageBackend,
    trash::{Moved, TrashError},
};
use tracing::debug;

use crate::{
    cli::CommandArgs,
    cmd::{
        self, ConversationLoadRequest, Output,
        conversation_id::{ConversationIds as _, PositionalIds},
        target::ConversationTarget,
    },
    ctx::Ctx,
    error::Error,
};

/// Move conversations to the trash.
///
/// Each conversation's directories, in every storage root, move to that root's
/// `conversations/.trash/`, where nothing else reads them.
/// `jp conversation untrash` moves them back.
#[derive(Debug)]
pub(crate) struct Trash {
    target: PositionalIds<false, true>,
}

/// Move conversations back out of the trash.
#[derive(Debug)]
pub(crate) struct Untrash {
    target: PositionalIds<false, true>,
}

impl CommandArgs for Trash {
    fn augment(cmd: Command) -> Command {
        required_ids(cmd)
    }

    fn from_matches(matches: &ArgMatches) -> Result<Self, clap::Error> {
        Ok(Self {
            target: PositionalIds::from_matches(matches)?,
        })
    }
}

impl CommandArgs for Untrash {
    fn augment(cmd: Command) -> Command {
        required_ids(cmd)
    }

    fn from_matches(matches: &ArgMatches) -> Result<Self, clap::Error> {
        Ok(Self {
            target: PositionalIds::from_matches(matches)?,
        })
    }
}

impl Trash {
    #[expect(clippy::unused_self)]
    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        // The conversations this command exists for are not in the index.
        ConversationLoadRequest::none()
    }

    pub(crate) fn run(self, ctx: &mut Ctx) -> Output {
        let now = ctx.now();
        run_batch(ctx, &self.target, Action::Trash, |fs, id| {
            fs.trash_conversation(id, now)
        })
    }
}

impl Untrash {
    #[expect(clippy::unused_self)]
    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        // Trashed conversations are not in the index.
        ConversationLoadRequest::none()
    }

    pub(crate) fn run(self, ctx: &mut Ctx) -> Output {
        run_batch(ctx, &self.target, Action::Untrash, |fs, id| {
            fs.untrash_conversation(id)
        })
    }
}

/// The positional IDs, at least one of them.
fn required_ids(cmd: Command) -> Command {
    PositionalIds::<false, true>::augment(cmd).mut_arg("id", |arg| arg.required(true))
}

#[derive(Debug, Clone, Copy)]
enum Action {
    Trash,
    Untrash,
}

impl Action {
    const fn command(self) -> &'static str {
        match self {
            Self::Trash => "trash",
            Self::Untrash => "untrash",
        }
    }

    /// What happened to a conversation, after "Conversation <id>".
    const fn done(self) -> &'static str {
        match self {
            Self::Trash => "moved to the trash",
            Self::Untrash => "restored from the trash",
        }
    }

    /// What did not happen to a conversation, after "Conversation <id>".
    const fn not_done(self) -> &'static str {
        match self {
            Self::Trash => "not moved to the trash",
            Self::Untrash => "not restored from the trash",
        }
    }

    /// The action after "Failed to", around the count.
    const fn failed(self) -> (&'static str, &'static str) {
        match self {
            Self::Trash => ("move", "to the trash"),
            Self::Untrash => ("restore", "from the trash"),
        }
    }
}

/// Apply `action` to every targeted conversation, reporting each one.
///
/// A conversation that fails is reported on stderr and the rest still run; the
/// command fails at the end if any did.
fn run_batch(
    ctx: &Ctx,
    target: &PositionalIds<false, true>,
    action: Action,
    apply: impl Fn(&FsStorageBackend, &ConversationId) -> Result<Vec<Moved>, TrashError>,
) -> Output {
    if !ctx.term.args.persist {
        return Err(format!(
            "`jp conversation {}` moves files, so it does not run with `--no-persist`.",
            action.command()
        )
        .into());
    }
    let Some(fs) = ctx.fs_backend.as_deref() else {
        return Err("This workspace has no conversation storage on disk.".into());
    };

    let ids = resolve_ids(ctx, target, action)?;
    let mut failed = 0_usize;
    for id in &ids {
        let name = id.to_string().bold().yellow();
        match apply(fs, id) {
            Ok(moved) => {
                for Moved { from, to } in &moved {
                    debug!(%id, %from, %to, "Moved conversation directory.");
                }
                ctx.printer
                    .println(format!("Conversation {name} {}.", action.done()));
            }
            Err(error) => {
                failed += 1;
                ctx.printer.error_println(format!(
                    "Conversation {name} {}: {error}.",
                    action.not_done()
                ));
            }
        }
    }

    if failed == 0 {
        return Ok(());
    }

    let noun = if ids.len() == 1 {
        "conversation"
    } else {
        "conversations"
    };
    let (verb, direction) = action.failed();
    Err(format!(
        "Failed to {verb} {failed} of {} {noun} {direction}.",
        ids.len()
    )
    .into())
}

/// The targeted IDs: typed IDs as they are, keywords resolved against the
/// conversation index.
///
/// The picker is refused: it lists the index, which holds none of the
/// conversations these commands exist for.
fn resolve_ids(
    ctx: &Ctx,
    target: &PositionalIds<false, true>,
    action: Action,
) -> Result<Vec<ConversationId>, cmd::Error> {
    let mut ids = vec![];
    for target in target.ids() {
        match target {
            ConversationTarget::Id(id) => ids.push(*id),
            ConversationTarget::Help => {
                return Err(Error::TargetHelp {
                    session: false,
                    multi: true,
                }
                .into());
            }
            ConversationTarget::Picker(_) => {
                return Err(format!(
                    "`jp conversation {}` takes conversation IDs; it has no picker.",
                    action.command()
                )
                .into());
            }
            other => ids.extend(other.resolve(&ctx.workspace, ctx.session.as_ref())?),
        }
    }
    Ok(ids)
}

#[cfg(test)]
#[path = "trash_tests.rs"]
mod tests;
