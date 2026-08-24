//! `jp conversation label`: manage the labels on a conversation.
//!
//! Keys and `key=value` pairs are bare arguments, so the shell splits them and
//! a value may contain any character, commas included.
//! The conversation is named with `--id`, which is accepted on either side of
//! the verb.

use crossterm::style::Stylize as _;
use jp_conversation::ConversationId;
use jp_inquire::prompt::TerminalPromptBackend;
use jp_printer::Printer;
use jp_workspace::ConversationHandle;
use serde_json::{Value, json};

use crate::{
    cmd::{
        ConversationLoadRequest, Output,
        conversation_id::FlagIds,
        label::{self, Change, LabelChange, LabelOperand, resolve::Resolver},
        lock::{LockOutcome, LockRequest, acquire_lock},
    },
    ctx::Ctx,
    error::Error,
    format::{label_detail_items, label_line, label_lines, shown_values},
    output::print_json,
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
    /// Add values to the conversation's labels.
    ///
    /// Values accumulate: a key that already holds values keeps them.
    /// Use `set` to replace what a key holds.
    #[command(name = "add", visible_alias = "a")]
    Add(Values),

    /// Replace the values of the labels named.
    ///
    /// Only the keys named are replaced; every other label is left alone.
    /// A bare `rm` clears them all.
    #[command(name = "set", visible_alias = "s")]
    Set(Values),

    /// Remove labels from the conversation.
    ///
    /// With no arguments, removes every label.
    #[command(name = "rm", visible_alias = "r", aliases = ["remove", "del", "delete"])]
    Rm(Rm),

    /// List the conversation's labels.
    #[command(name = "ls", alias = "list")]
    Ls,
}

#[derive(Debug, clap::Args)]
pub(crate) struct Values {
    /// The labels to apply, as `key=value` or a bare `key` for an empty value.
    ///
    /// `:name` resolves the `conversation.labels.name` rule and applies
    /// whatever it produces.
    /// Values are taken literally, so they may contain commas.
    /// Naming one key several times acts on all of its values at once.
    #[arg(value_name = "KEY[=VALUE]|:NAME", required = true)]
    labels: Vec<String>,
}

#[derive(Debug, clap::Args)]
pub(crate) struct Rm {
    /// The labels to remove, as a bare `key` or a `key=value` pair.
    ///
    /// A bare key removes every value it holds.
    /// With none given, every label is removed.
    ///
    /// Removing a label the conversation doesn't carry is not an error, but it
    /// is reported.
    #[arg(value_name = "KEY[=VALUE]")]
    labels: Vec<String>,
}

impl Label {
    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        let mut request = ConversationLoadRequest::explicit_or_session(&self.target);

        // Loading the target's config is what makes `:name` resolvable: the
        // rule comes from that conversation's effective config, not the
        // workspace's.
        if matches!(self.command, Some(Commands::Add(_) | Commands::Set(_))) {
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

        // An alias names a rule to resolve into a value, so it makes sense
        // where values are being applied. `rm` names values already stored, so
        // `:name` is nothing but an invalid key there.
        let (verb, raw, aliases) = match &command {
            Commands::Add(args) => (Verb::Add, &args.labels, true),
            Commands::Set(args) => (Verb::Set, &args.labels, true),
            Commands::Rm(args) => (Verb::Remove, &args.labels, false),
            Commands::Ls => unreachable!("handled above"),
        };

        // A bare `rm` clears everything. Unlike a flag with an optional value,
        // the argument slot here holds only labels, so an empty slot cannot
        // swallow the conversation target.
        let remove_all = verb == Verb::Remove && raw.is_empty();

        let operands = raw
            .iter()
            .map(|raw| LabelOperand::parse(raw, aliases))
            .collect::<Result<Vec<_>, _>>()
            .map_err(Error::Label)?;

        // Only the first handle's config is layered, so an alias resolved
        // against it would be wrong for every other target.
        let aliased = operands.iter().any(|o| o.as_alias().is_some());
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
        let mut report = Report::new(verb);

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
            let resolved = label::expand_aliases(&operands, &resolver).await?;

            let change = match verb {
                Verb::Add => LabelChange::Add(resolved.grouped()),
                Verb::Set => LabelChange::Set(resolved.grouped()),
                Verb::Remove if remove_all => LabelChange::RemoveAll,
                Verb::Remove => LabelChange::Remove(resolved.grouped_for_removal()),
            };

            let applied = lock
                .as_mut()
                .update_metadata(|m| label::apply(&mut m.labels, &change));
            label::report_missing(&ctx.printer, lock.id(), &applied.missing);

            report.record(&applied.changes);

            let id = lock.id();
            let title = lock.metadata().title.clone();

            if ctx.printer.format().is_json() {
                print_json(
                    &ctx.printer,
                    &report.json(&applied.changes, id, title.as_deref()),
                );
                continue;
            }

            // Which conversation changed is chrome; the labels are the data a
            // script reads, so they go to stdout whether or not the chrome
            // collapsed.
            if !collapse {
                let target = conversation_target(id, title.as_deref());
                report.print_chrome(&ctx.printer, &applied.changes, &target);
            }

            for line in diff_lines(&applied.changes) {
                ctx.printer.println(line);
            }
        }

        if collapse && !ctx.printer.format().is_json() {
            let target = format!("{} conversations", count.to_string().bold().yellow());
            ctx.printer.eprintln(report.collapsed_line(&target));
        }

        Ok(())
    }
}

/// How many conversations are named individually before the report collapses to
/// a count.
const MAX_LISTED_TARGETS: usize = 6;

/// The verb an invocation ran, which decides how its outcome reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verb {
    Add,
    Set,
    Remove,
}

impl Verb {
    /// The verb as it is written on the command line, for machine output.
    const fn as_str(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Set => "set",
            Self::Remove => "rm",
        }
    }
}

/// Accumulates what an invocation did, so it can be reported per conversation
/// or as one collapsed line.
#[derive(Debug)]
struct Report {
    /// The verb that produced the changes.
    verb: Verb,

    /// Every key touched across all targets, in first-seen order.
    ///
    /// Only used for the collapsed line: with one target per line the exact
    /// per-target change is reported instead.
    keys: Vec<String>,
}

impl Report {
    const fn new(verb: Verb) -> Self {
        Self { verb, keys: vec![] }
    }

    /// Record one target's outcome for the collapsed line.
    fn record(&mut self, changes: &[Change]) {
        for change in changes {
            if !self.keys.contains(&change.key) {
                self.keys.push(change.key.clone());
            }
        }
    }

    /// Print one target's chrome, followed by a blank line when labels follow
    /// it on stdout.
    fn print_chrome(&self, printer: &Printer, changes: &[Change], target: &str) {
        printer.eprintln(self.chrome(changes, target));

        if !changes.is_empty() {
            printer.eprintln("");
        }
    }

    /// What happened to one target, for the reader rather than the script.
    fn chrome(&self, changes: &[Change], target: &str) -> String {
        if changes.is_empty() {
            return self.nothing_line(target);
        }

        match self.verb {
            Verb::Add => format!("Added labels to {target}"),
            Verb::Set => format!("Set labels on {target}"),
            Verb::Remove => format!("Removed labels from {target}"),
        }
    }

    /// The reported line for several targets at once, which names the keys
    /// touched rather than each conversation's own before-and-after.
    fn collapsed_line(&self, target: &str) -> String {
        if self.keys.is_empty() {
            return self.nothing_line(target);
        }

        let list = self.keys.join(", ").bold();
        match self.verb {
            Verb::Add => format!("Added labels {list} to {target}"),
            Verb::Set => format!("Set labels {list} on {target}"),
            Verb::Remove => format!("Removed labels {list} from {target}"),
        }
    }

    /// The line for an invocation that changed nothing.
    ///
    /// A removal that matched nothing says so rather than printing an empty
    /// list; the per-key warnings alongside it explain why.
    fn nothing_line(&self, target: &str) -> String {
        match self.verb {
            Verb::Add | Verb::Set => format!("No labels to apply to {target}"),
            Verb::Remove => format!("No labels to remove from {target}"),
        }
    }

    /// The machine-readable form of one target's outcome.
    ///
    /// An empty `changes` array is an invocation that matched nothing; there is
    /// no prose equivalent to parse.
    fn json(&self, changes: &[Change], id: ConversationId, title: Option<&str>) -> Value {
        json!({
            "action": self.verb.as_str(),
            "conversation": { "id": id.to_string(), "title": title },
            "changes": changes
                .iter()
                .map(|change| json!({
                    "key": change.key,
                    "before": shown_values(&change.before),
                    "after": shown_values(&change.after),
                }))
                .collect::<Vec<_>>(),
        })
    }
}

/// Render what a mutation did as diff lines, one label per line.
///
/// `-` marks a value the key lost, `+` one it gained, and a space one it kept,
/// which is the same marker column a listing prints.
/// Reading the same event the same way whatever the verb was means a line can
/// be understood without knowing which verb produced it.
fn diff_lines(changes: &[Change]) -> Vec<String> {
    changes
        .iter()
        .flat_map(|change| {
            // The values the key held, in order, each kept or lost; then the
            // ones it gained, which is how a diff of two ordered sets reads.
            let mut lines: Vec<String> = change
                .before
                .iter()
                .map(|value| {
                    let marker = if change.after.contains(value) {
                        ' '
                    } else {
                        '-'
                    };
                    label_line(marker, &change.key, value)
                })
                .collect();

            lines.extend(
                change
                    .after
                    .iter()
                    .filter(|value| !change.before.contains(*value))
                    .map(|value| label_line('+', &change.key, value)),
            );

            lines
        })
        .collect()
}

/// Render a conversation as `<id>: <title>`, matching `jp c use`.
fn conversation_target(id: ConversationId, title: Option<&str>) -> String {
    let id = id.to_string().bold().yellow();
    match title {
        Some(title) => format!("{id}: {}", title.yellow()),
        None => id.to_string(),
    }
}

/// Print each conversation's labels, one `key=value` per line.
///
/// The lines are the data, so they go to stdout with nothing around them: a
/// reader needs no context from the lines before or after.
fn list(ctx: &Ctx, handles: &[ConversationHandle]) -> Output {
    if handles.is_empty() {
        return Err(Error::NoConversationTarget.into());
    }

    // Which conversation a label belongs to is only in question when several
    // were named, and it is chrome either way.
    let named = handles.len() > 1;

    for handle in handles {
        let id = handle.id();
        let conversation = ctx.workspace.metadata(handle)?;

        if ctx.printer.format().is_json() {
            let items = label_detail_items(&conversation.labels);
            print_json(
                &ctx.printer,
                &json!({
                    "conversation": { "id": id.to_string(), "title": conversation.title },
                    "labels": items.iter().map(|item| item.json.clone()).collect::<Vec<_>>(),
                }),
            );
            continue;
        }

        if named {
            ctx.printer
                .eprintln(id.to_string().bold().yellow().to_string());
        }

        let lines = label_lines(&conversation.labels);
        if lines.is_empty() {
            ctx.printer.eprintln("No labels.");
            continue;
        }

        for line in lines {
            ctx.printer.println(line);
        }
    }

    Ok(())
}

#[cfg(test)]
#[path = "label_tests.rs"]
mod tests;
