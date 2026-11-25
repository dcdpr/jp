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
use jp_workspace::Workspace;
use tracing::{debug, trace, warn};
use url::Url;

use super::Output;
use crate::{
    IntoPartialAppConfig,
    cli::{CommandArgs, subcommand_of, unread_subcommand},
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
                    .about("Add attachment to context")
                    .alias("a"),
            )
            .subcommand(
                rm::Rm::augment(Command::new("rm"))
                    .about("Remove attachment from context")
                    .alias("r"),
            )
            .subcommand(
                ls::Ls::augment(Command::new("ls"))
                    .about("List attachments in context")
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
    pub(crate) async fn run(self, ctx: &mut Ctx) -> Output {
        match self.command {
            Commands::Add(args) => args.run(ctx),
            Commands::Remove(args) => args.run(ctx),
            Commands::List(args) => args.run(ctx),
            Commands::Print(args) => args.run(ctx).await,
        }
    }
}

impl IntoPartialAppConfig for Attachment {
    fn apply_cli_config(
        &self,
        workspace: Option<&Workspace>,
        partial: PartialAppConfig,
        merged_config: Option<&PartialAppConfig>,
    ) -> std::result::Result<PartialAppConfig, Box<dyn std::error::Error + Send + Sync>> {
        match &self.command {
            Commands::Add(args) => args.apply_cli_config(workspace, partial, merged_config),
            Commands::Remove(args) => args.apply_cli_config(workspace, partial, merged_config),
            Commands::List(_) | Commands::Print(_) => Ok(partial),
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

fn attachment_scheme(uri: &Url) -> &str {
    uri.scheme()
        .split_once('+')
        .map_or(uri.scheme(), |(scheme, _)| scheme)
}

/// Return the URL `uri` resolves as, in the single spelling its handler writes.
///
/// Two attachments with the same canonical URL resolve to the same content.
/// No I/O is performed, so a URL can be checked before it is stored.
///
/// # Errors
///
/// Returns an error when no handler serves the URL's scheme, or the handler
/// cannot resolve the URL.
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

/// The URL attachments are compared by: the canonical URL of `uri`, or `uri` as
/// written when it has none.
///
/// An entry no handler can resolve still fails when it is resolved; comparing
/// it as written only keeps it from being taken for another entry.
pub(crate) fn attachment_identity(uri: &Url) -> Url {
    canonical_attachment(uri).unwrap_or_else(|_| uri.clone())
}

/// Append `urls` to the conversation config, skipping every URL the config
/// already attaches.
///
/// Attachments are compared by canonical URL (see [`canonical_attachment`]): a
/// URL is skipped when an entry already attaches it in any spelling its handler
/// treats alike, including a config table that builds it.
/// A URL that is attached is stored as written.
///
/// # Errors
///
/// Returns an error for a URL [`canonical_attachment`] rejects, before any URL
/// is attached.
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
            AttachmentConfig::from_partial(attachment.clone(), vec![])
                .ok()?
                .to_url()
                .ok()
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
/// A URL given more than once, in any spelling its handler treats alike, is
/// resolved once, at its first position (see [`attachment_identity`]).
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
