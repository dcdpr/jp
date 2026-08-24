//! `jp conversation label`: manage the labels on a conversation.
//!
//! Keys and `key=value` pairs are bare arguments, so the shell splits them and
//! a value may contain any character, commas included.
//! The conversation is named with `--id`, which is accepted on either side of
//! the verb.

use crossterm::style::Stylize as _;
use jp_conversation::{Conversation, ConversationId};
use jp_inquire::prompt::TerminalPromptBackend;
use jp_term::table::{DetailItem, Details};
use jp_workspace::ConversationHandle;
use serde_json::{Value, json};

use crate::{
    cmd::{
        ConversationLoadRequest, Output,
        conversation_id::FlagIds,
        label::{self, LabelDirective, resolve::Resolver},
        lock::{LockOutcome, LockRequest, acquire_lock},
    },
    ctx::Ctx,
    error::Error,
    format::{label_detail_item, label_text},
    output::{print_details_with_json, print_outcome},
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
    ///
    /// With no keys, removes every label.
    #[command(name = "rm", visible_alias = "r", aliases = ["remove", "del", "delete"])]
    Rm(Rm),

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
    /// With none given, every label is removed.
    ///
    /// Removing a key the conversation doesn't carry is not an error, but it is
    /// reported.
    #[arg(value_name = "KEY")]
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
            // A bare `rm` clears everything. Unlike a flag with an optional
            // value, the argument slot here holds only label keys, so an empty
            // slot cannot swallow the conversation target.
            Commands::Rm(args) if args.keys.is_empty() => Ok(vec![LabelDirective::RemoveAll]),
            Commands::Rm(args) => args
                .keys
                .iter()
                .map(|raw| LabelDirective::parse_remove(raw))
                .collect::<Result<Vec<_>, _>>(),
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

        // Past a handful of targets, one line each is noise for a reader.
        // A machine reader wants every record, so JSON never collapses.
        let collapse = !ctx.printer.format().is_json() && handles.len() > MAX_LISTED_TARGETS;
        let count = handles.len();
        let mut report = Report::new(matches!(command, Commands::Add(_)));

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

            let applied = lock
                .as_mut()
                .update_metadata(|m| label::apply(&mut m.labels, &directives));
            label::report_missing(&ctx.printer, lock.id(), &applied.missing);

            let labels = report.touched(&directives, &applied);

            if !collapse {
                let id = lock.id();
                let title = lock.metadata().title.clone();
                print_outcome(
                    &ctx.printer,
                    &report.line(&labels, &conversation_target(id, title.as_deref())),
                    &report.json(&labels, id, title.as_deref()),
                );
            }
        }

        if collapse {
            let target = format!("{} conversations", count.to_string().bold().yellow());
            let labels = report.union.clone();
            ctx.printer.println(report.line(&labels, &target));
        }

        Ok(())
    }
}

/// How many conversations are named individually before the report collapses to
/// a count.
const MAX_LISTED_TARGETS: usize = 6;

/// Accumulates what an invocation did, so it can be reported per conversation
/// or as one collapsed line.
#[derive(Debug)]
struct Report {
    /// Whether the invocation added labels; otherwise it removed them.
    added: bool,

    /// Every label touched across all targets, in first-seen order.
    ///
    /// Only used for the collapsed line: with one target per line the exact
    /// per-target set is reported instead.
    union: Vec<(String, String)>,
}

impl Report {
    const fn new(added: bool) -> Self {
        Self {
            added,
            union: vec![],
        }
    }

    /// Record one target's outcome and return the labels to name for it.
    ///
    /// Additions are taken from the directives, since every named label is set
    /// whether or not it was already there.
    /// Removals are taken from what was actually in the map, so the reported
    /// line lists real values rather than the keys the user asked about.
    fn touched(
        &mut self,
        directives: &label::Resolved,
        applied: &label::Applied,
    ) -> Vec<(String, String)> {
        let labels: Vec<(String, String)> = if self.added {
            directives
                .iter()
                .filter_map(|d| match d {
                    LabelDirective::Set { key, value } => Some((key.clone(), value.clone())),
                    _ => None,
                })
                .collect()
        } else {
            applied.removed.clone()
        };

        for label in &labels {
            if !self.union.contains(label) {
                self.union.push(label.clone());
            }
        }

        labels
    }

    /// The reported line for one target, which is either a named conversation
    /// or a count.
    ///
    /// A removal that matched nothing says so rather than printing an empty
    /// list; the per-key warnings above explain why.
    fn line(&self, labels: &[(String, String)], target: &str) -> String {
        if labels.is_empty() {
            return format!("No labels to remove from {target}");
        }

        let list = labels
            .iter()
            .map(|(key, value)| label_text(key, value))
            .collect::<Vec<_>>()
            .join(", ")
            .bold();

        if self.added {
            format!("Added labels {list} to {target}")
        } else {
            format!("Removed labels {list} from {target}")
        }
    }

    /// The machine-readable form of one target's outcome.
    ///
    /// Labels carry the same `{key, value}` shape `jp c show` emits, so a
    /// script reads both commands the same way.
    /// An empty array is the removal that matched nothing; there is no prose
    /// equivalent to parse.
    fn json(&self, labels: &[(String, String)], id: ConversationId, title: Option<&str>) -> Value {
        json!({
            "action": if self.added { "added" } else { "removed" },
            "conversation": { "id": id.to_string(), "title": title },
            "labels": labels
                .iter()
                .map(|(key, value)| label_detail_item(key, value).json)
                .collect::<Vec<_>>(),
        })
    }
}

/// Render a conversation as `<id>: <title>`, matching `jp c use`.
fn conversation_target(id: ConversationId, title: Option<&str>) -> String {
    let id = id.to_string().bold().yellow();
    match title {
        Some(title) => format!("{id}: {}", title.yellow()),
        None => id.to_string(),
    }
}

/// Print each conversation's labels.
fn list(ctx: &Ctx, handles: &[ConversationHandle]) -> Output {
    if handles.is_empty() {
        return Err(Error::NoConversationTarget.into());
    }

    for handle in handles {
        let id = handle.id();
        let conversation = ctx.workspace.metadata(handle)?;

        let json = json!({
            "conversation": { "id": id.to_string(), "title": conversation.title },
            "labels": conversation
                .labels
                .iter()
                .map(|(key, value)| label_detail_item(key, value).json)
                .collect::<Vec<_>>(),
        });

        print_details_with_json(
            &ctx.printer,
            Some(&id.to_string()),
            label_rows(&conversation),
            &json,
        );
    }

    Ok(())
}

#[cfg(test)]
#[path = "label_tests.rs"]
mod tests;

/// The conversation's labels, for the text views.
///
/// A listing rather than named fields: label keys are user-chosen, so they are
/// data rather than a fixed set of properties.
/// The empty case says so in words; the JSON form is supplied separately and
/// stays an empty array.
fn label_rows(conversation: &Conversation) -> Details {
    if conversation.labels.is_empty() {
        return Details::Items(vec![DetailItem::plain("No labels.")]);
    }

    Details::Items(
        conversation
            .labels
            .iter()
            .map(|(key, value)| label_detail_item(key, value))
            .collect(),
    )
}
