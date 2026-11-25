use std::collections::HashSet;

use clap::{ArgMatches, Command};
use jp_attachment_agentic_shepherd as _;
use jp_attachment_bear_note as _;
use jp_attachment_cmd_output as _;
use jp_attachment_file_content as _;
use jp_attachment_github as _;
use jp_attachment_http_content as _;
use jp_attachment_internal::{
    ResolveError, canonical as canonical_internal_attachment,
    resolve as resolve_internal_attachment,
};
use jp_attachment_mcp_resources as _;
use jp_config::{Config as _, PartialAppConfig, conversation::attachment::AttachmentConfig};
use jp_conversation::stream::ApplyDelta;
use jp_workspace::{ConversationHandle, ConversationLock};
use tracing::{debug, trace, warn};
use url::Url;

use super::{
    ConversationLoadRequest, Output,
    lock::{LockOutcome, LockRequest, acquire_lock},
    target::TargetGrammar,
};
use crate::{
    cli::{CommandArgs, subcommand_of, unread_subcommand},
    config_pipeline::override_to_record,
    ctx::Ctx,
    error::{Error, Result},
};

pub(super) mod add;
mod ls;
mod print;
mod rm;

/// `jp attachment`.
#[derive(Debug)]
pub(crate) struct Attachment {
    command: Commands,
}

impl CommandArgs for Attachment {
    fn augment(cmd: Command) -> Command {
        cmd.subcommand_required(true)
            .arg_required_else_help(true)
            .subcommand(
                add::Add::augment(Command::new("add"))
                    .about("Add attachments to a conversation")
                    .alias("a"),
            )
            .subcommand(
                rm::Rm::augment(Command::new("rm"))
                    .about("Remove attachments from a conversation")
                    .alias("r"),
            )
            .subcommand(
                ls::Ls::augment(Command::new("ls"))
                    .about("List a conversation's attachments")
                    .alias("l"),
            )
            .subcommand(
                print::Print::augment(Command::new("print"))
                    .about("Preview how an attachment will render for the LLM")
                    .alias("p"),
            )
    }

    fn from_matches(matches: &ArgMatches) -> std::result::Result<Self, clap::Error> {
        let command = match subcommand_of(matches)? {
            ("add", sub) => Commands::Add(add::Add::from_matches(sub)?),
            ("rm", sub) => Commands::Remove(rm::Rm::from_matches(sub)?),
            ("ls", sub) => Commands::List(ls::Ls::from_matches(sub)?),
            ("print", sub) => Commands::Print(print::Print::from_matches(sub)?),
            (name, _) => return Err(unread_subcommand(name)),
        };

        Ok(Self { command })
    }
}

impl Attachment {
    pub(crate) async fn run(self, ctx: &mut Ctx, handles: Vec<ConversationHandle>) -> Output {
        match self.command {
            Commands::Add(args) => args.run(ctx, handles).await,
            Commands::Remove(args) => args.run(ctx, handles).await,
            Commands::List(args) => args.run(ctx, &handles),
            Commands::Print(args) => args.run(ctx).await,
        }
    }

    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        match &self.command {
            Commands::Add(args) => args.conversation_load_request(),
            Commands::Remove(args) => args.conversation_load_request(),
            Commands::List(args) => args.conversation_load_request(),
            Commands::Print(_) => ConversationLoadRequest::none(),
        }
    }
}

#[derive(Debug)]
enum Commands {
    Add(add::Add),
    Remove(rm::Rm),
    List(ls::Ls),
    Print(print::Print),
}

/// Lock the conversation `handles` resolved to.
///
/// Returns the target error for `grammar` when no conversation was resolved.
async fn lock_target(
    ctx: &Ctx,
    handles: Vec<ConversationHandle>,
    grammar: TargetGrammar,
) -> Result<ConversationLock> {
    let Some(handle) = handles.into_iter().next() else {
        return Err(Error::from(grammar));
    };

    let id = handle.id();
    match acquire_lock(LockRequest::from_ctx(handle, ctx)).await? {
        LockOutcome::Acquired(lock) => Ok(lock),
        // The request offers neither choice, so the lock was not acquired.
        LockOutcome::NewConversation | LockOutcome::ForkConversation(_) => {
            Err(Error::LockTimeout(id))
        }
    }
}

/// Record `overrides` as a config delta on the conversation behind `lock`, if
/// they change its configuration.
fn record_overrides(ctx: &Ctx, lock: ConversationLock, overrides: PartialAppConfig) -> Result<()> {
    let current = lock
        .events()
        .config_partial()
        .map_err(jp_conversation::Error::from)?;

    let Some((delta, unsets)) = override_to_record(&current, overrides)? else {
        return Ok(());
    };

    let apply = ApplyDelta::with_unsets(ctx.now(), delta, unsets);
    lock.into_mut()
        .update_events_and_flush(|events| events.add_config_delta(apply))?;

    Ok(())
}

fn attachment_scheme(uri: &Url) -> &str {
    uri.scheme()
        .split_once('+')
        .map_or(uri.scheme(), |(scheme, _)| scheme)
}

/// The URL an attachment config names.
///
/// A table's URL is built by the handler for its `type`, which knows where
/// `path` belongs in that URL, with `params` added as query parameters.
/// A type no handler serves, such as `jp`, gets the generic `<type>://<path>`.
pub(crate) fn attachment_url(attachment: &AttachmentConfig) -> Result<Url> {
    let AttachmentConfig::Object(table) = attachment else {
        return Ok(attachment.to_url()?);
    };

    let Some(handler) = jp_attachment::find_handler_by_scheme(&table.kind) else {
        return Ok(table.to_url()?);
    };

    let url = handler.table_url(&table.path).map_err(|error| {
        Error::Attachment(format!(
            "Invalid `{}` attachment path `{}`: {error}",
            table.kind, table.path
        ))
    })?;

    Ok(table.with_params(url)?)
}

/// Validate and canonicalize `uri` via its scheme's attachment handler.
///
/// Two attachments with the same canonical URL resolve to the same content.
/// Performs no I/O, so callers can validate a URL before storing it.
///
/// # Errors
///
/// Returns an error if no handler is registered for the scheme or if the
/// handler rejects `uri`.
pub(crate) fn canonical_attachment(uri: &Url) -> Result<Url> {
    trace!(%uri, "Canonicalizing attachment.");

    let scheme = attachment_scheme(uri);
    let canonical = if scheme == "jp" {
        canonical_internal_attachment(uri)
    } else {
        let Some(handler) = jp_attachment::find_handler_by_scheme(scheme) else {
            return Err(Error::NotFound("Attachment handler", scheme.to_string()));
        };

        handler.canonical(uri)
    };

    canonical.map_err(|source| Error::AttachmentFailed {
        uri: uri.clone(),
        source,
    })
}

/// Return the deduplication key for `uri`: its canonical URL, or `uri` verbatim
/// if canonicalization fails.
///
/// Falling back to the raw URL ensures an unresolvable entry is not conflated
/// with a distinct entry; it still fails when resolved.
pub(crate) fn attachment_identity(uri: &Url) -> Url {
    canonical_attachment(uri).unwrap_or_else(|_| uri.clone())
}

/// Append `urls` to the conversation config, skipping URLs that are already
/// attached.
///
/// Deduplication compares canonical URLs (see [`canonical_attachment`]), so an
/// equivalent URL spelling or config-table entry is skipped.
/// Newly attached URLs are stored as written.
///
/// # Errors
///
/// Returns an error if [`canonical_attachment`] rejects any URL in `urls`,
/// before modifying `partial`.
pub(crate) fn attach_urls(
    partial: &mut PartialAppConfig,
    urls: impl IntoIterator<Item = Url>,
) -> Result<()> {
    let urls = urls
        .into_iter()
        .map(|url| Ok((canonical_attachment(&url)?, url)))
        .collect::<Result<Vec<_>>>()?;

    let attachments = &mut partial.conversation.attachments;
    let mut attached: Vec<Url> = attachments
        .iter()
        .filter_map(|attachment| {
            attachment_url(&AttachmentConfig::from_partial(attachment.clone(), vec![]).ok()?).ok()
        })
        .map(|url| attachment_identity(&url))
        .collect();

    for (canonical, url) in urls {
        if attached.contains(&canonical) {
            continue;
        }

        attached.push(canonical);
        attachments.push(url.into());
    }

    Ok(())
}

pub(crate) async fn register_attachment(
    ctx: &Ctx,
    uri: Url,
) -> Result<Vec<jp_attachment::Attachment>> {
    trace!(uri = uri.as_str(), "Registering attachment.");

    let scheme = attachment_scheme(&uri);

    if scheme == "jp" {
        return match resolve_internal_attachment(&ctx.workspace, &uri) {
            Ok(attachments) => Ok(attachments),
            Err(ResolveError::ConversationMissing(id)) => {
                Err(Error::AttachmentConversationMissing { id, uri })
            }
            Err(ResolveError::Other(source)) => Err(Error::AttachmentFailed { uri, source }),
        };
    }

    let Some(mut handler) = jp_attachment::find_handler_by_scheme(scheme) else {
        return Err(Error::NotFound("Attachment handler", scheme.to_string()));
    };

    handler
        .add(&uri, ctx.workspace.root())
        .await
        .map_err(|source| Error::AttachmentFailed {
            uri: uri.clone(),
            source,
        })?;

    handler
        .get(ctx.workspace.root())
        .await
        .map_err(|source| Error::AttachmentFailed { uri, source })
}

/// Resolve a list of attachment URLs for the current query.
///
/// Unlike [`register_attachment`], this loader is tolerant of `jp://`
/// attachments whose target conversation has been archived or removed since the
/// attachment was registered: those references are warned about and skipped
/// rather than aborting the whole query.
/// Every other failure (invalid URI, real I/O error, etc.) is propagated.
///
/// Duplicate URLs (including equivalent spellings that normalize to the same
/// [`attachment_identity`]) are resolved once, at their first position.
///
/// Returns one group per input URL in order; a skipped reference or duplicate
/// URL yields an empty group to keep the output aligned with `urls`.
pub(crate) async fn load_conversation_attachments(
    ctx: &Ctx,
    urls: Vec<Url>,
) -> Result<Vec<Vec<jp_attachment::Attachment>>> {
    // Stored conversation configs can contain duplicate URLs, and each URL
    // instantiates its own handler in `register_attachment`, so deduplicate
    // across the batch here.
    let mut seen = HashSet::new();

    // Handle the missing-conversation case inside each future so the outer
    // `try_join_all` keeps its fail-fast behavior for real errors: a
    // structural failure aborts the batch immediately instead of waiting
    // for slow HTTP/GitHub handlers to finish.
    let futs = urls.into_iter().map(|url| {
        let repeated = !seen.insert(attachment_identity(&url));

        async move {
            if repeated {
                debug!(uri = %url, "Skipping attachment: URL is attached more than once.");
                return Ok(Vec::new());
            }

            match register_attachment(ctx, url).await {
                Ok(atts) => Ok(atts),
                Err(Error::AttachmentConversationMissing { id, uri }) => {
                    warn!(
                        %id,
                        %uri,
                        "Skipping attachment: referenced conversation is unavailable."
                    );
                    Ok(Vec::new())
                }
                Err(error) => Err(error),
            }
        }
    });
    futures::future::try_join_all(futs).await
}

#[cfg(test)]
#[path = "attachment_tests.rs"]
mod tests;
