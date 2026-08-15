//! `jp conversation label`: manage the labels on a conversation.
//!
//! Keys and `key=value` pairs are bare arguments, so the shell splits them and
//! a value may contain any character, commas included.
//! The conversation is named with `--id`, which is accepted on either side of
//! the verb.

use jp_conversation::Conversation;
use jp_inquire::prompt::TerminalPromptBackend;
use jp_term::table::DetailItem;
use jp_workspace::ConversationHandle;

use crate::{
    cmd::{
        ConversationLoadRequest, Output,
        conversation_id::FlagIds,
        label::{self, LabelDirective, resolve::Resolver},
        lock::{LockOutcome, LockRequest, acquire_lock},
    },
    ctx::Ctx,
    error::Error,
    format::label_detail_item,
    output::print_details,
};

/// Manage labels on a conversation.
#[derive(Debug, clap::Args)]
pub(crate) struct Label {
    /// The conversation to act on.
    ///
    /// Accepted before or after the verb; defaults to the session's active
    /// conversation.
    #[command(flatten)]
    target: FlagIds<true, true, true>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, clap::Subcommand)]
enum Commands {
    /// Add labels to the conversation.
    #[command(name = "add", visible_alias = "a", alias = "set")]
    Add(Add),

    /// Remove labels from the conversation.
    #[command(name = "rm", visible_alias = "r", aliases = ["remove", "del", "delete"])]
    Rm(Rm),

    /// Remove every label from the conversation.
    #[command(name = "reset", alias = "clear")]
    Reset,

    /// List the conversation's labels.
    #[command(name = "ls", alias = "list")]
    Ls,
}

#[derive(Debug, clap::Args)]
pub(crate) struct Add {
    /// The labels to add, as `key=value` or a bare `key` for an empty value.
    ///
    /// `:name` resolves the `conversation.labels.name` rule and applies
    /// whatever it produces.
    /// Values are taken literally, so they may contain commas.
    #[arg(value_name = "KEY[=VALUE]|:NAME", required = true)]
    labels: Vec<String>,
}

#[derive(Debug, clap::Args)]
pub(crate) struct Rm {
    /// The label keys to remove.
    ///
    /// Removing a key the conversation doesn't carry is not an error, but it is
    /// reported.
    #[arg(value_name = "KEY", required = true)]
    keys: Vec<String>,
}

impl Label {
    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        let mut request = ConversationLoadRequest::explicit_or_session(&self.target);

        // Loading the target's config is what makes `add :name` resolvable: the
        // rule comes from that conversation's effective config, not the
        // workspace's.
        if matches!(self.command, Some(Commands::Add(_))) {
            request.config_conversation = Some(0);
        }

        request
    }

    pub(crate) async fn run(self, ctx: &mut Ctx, handles: Vec<ConversationHandle>) -> Output {
        // A bare `jp c label` lists, which is the only read-only verb and the
        // only one that accepts several conversations.
        let Some(command) = self.command else {
            return list(ctx, &handles);
        };

        if let Commands::Ls = command {
            return list(ctx, &handles);
        }

        let directives = match &command {
            Commands::Add(args) => args
                .labels
                .iter()
                .map(|raw| LabelDirective::parse_set::<true>(raw))
                .collect::<Result<Vec<_>, _>>(),
            Commands::Rm(args) => args
                .keys
                .iter()
                .map(|raw| LabelDirective::parse_remove(raw))
                .collect::<Result<Vec<_>, _>>(),
            Commands::Reset => Ok(vec![LabelDirective::RemoveAll]),
            Commands::Ls => unreachable!("handled above"),
        }
        .map_err(Error::Label)?;

        // Only the first handle's config is layered, so an alias resolved
        // against it would be wrong for every other target.
        let aliased = directives.iter().any(|d| d.as_alias().is_some());
        if aliased && handles.len() > 1 {
            return Err(Error::Label(
                "a label alias resolves against one conversation's configuration, so it cannot be \
                 applied to several at once; name a single conversation with `--id`"
                    .to_owned(),
            )
            .into());
        }

        if handles.is_empty() {
            return Err(Error::NoConversationTarget.into());
        }

        for handle in handles {
            let lock = match acquire_lock(LockRequest::from_ctx(handle, ctx)).await? {
                LockOutcome::Acquired(lock) => lock,
                LockOutcome::NewConversation | LockOutcome::ForkConversation(_) => {
                    unreachable!("neither is offered")
                }
            };

            // Resolved under the lock: an alias may run a command with side
            // effects, and running it only to fail on a conversation another
            // process holds would leave that effect unexplained.
            let config = ctx.config();
            let prompts = TerminalPromptBackend;
            let resolver = Resolver::new(
                &config.conversation.labels,
                ctx.workspace.root(),
                ctx.term.is_tty,
                &ctx.printer,
                &prompts,
            );
            let directives = label::expand_aliases(&directives, &resolver).await?;

            let missing = lock
                .as_mut()
                .update_metadata(|m| label::apply(&mut m.labels, &directives));
            label::report_missing(&ctx.printer, lock.id(), &missing);
        }

        ctx.printer.println("Conversation(s) updated.");
        Ok(())
    }
}

/// Print each conversation's labels.
fn list(ctx: &Ctx, handles: &[ConversationHandle]) -> Output {
    if handles.is_empty() {
        return Err(Error::NoConversationTarget.into());
    }

    for handle in handles {
        let conversation = ctx.workspace.metadata(handle)?;
        let rows = label_rows(&conversation);
        print_details(&ctx.printer, Some(&handle.id().to_string()), rows);
    }

    Ok(())
}

#[cfg(test)]
#[path = "label_tests.rs"]
mod tests;

/// One detail row per label, or a single "no labels" row.
fn label_rows(conversation: &Conversation) -> Vec<jp_term::table::DetailRow> {
    use jp_term::table::DetailRow;

    if conversation.labels.is_empty() {
        return vec![DetailRow::bare("No labels.")];
    }

    conversation
        .labels
        .iter()
        .map(|(key, value)| {
            let DetailItem { text, .. } = label_detail_item(key, value);
            DetailRow::bare(text)
        })
        .collect()
}
