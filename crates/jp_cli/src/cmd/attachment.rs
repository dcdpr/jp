use clap::{ArgMatches, Command};
use jp_attachment_agentic_shepherd as _;
use jp_attachment_bear_note as _;
use jp_attachment_cmd_output as _;
use jp_attachment_file_content as _;
use jp_attachment_github as _;
use jp_attachment_http_content as _;
use jp_attachment_internal::{
    ResolveError, resolve as resolve_internal_attachment, validate as validate_internal_attachment,
};
use jp_attachment_mcp_resources as _;
use jp_config::PartialAppConfig;
use jp_workspace::Workspace;
use tracing::{trace, warn};
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

pub(crate) fn validate_attachment(uri: &Url) -> Result<()> {
    trace!(%uri, "Validating attachment.");

    let scheme = attachment_scheme(uri);

    if scheme == "jp" {
        validate_internal_attachment(uri).map_err(|source| Error::AttachmentFailed {
            uri: uri.clone(),
            source,
        })?;
        return Ok(());
    }

    if jp_attachment::find_handler_by_scheme(scheme).is_none() {
        return Err(Error::NotFound("Attachment handler", scheme.to_string()));
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
/// Returns one group per URL, in the order the URLs were given; a skipped
/// reference yields an empty group, keeping the groups aligned with the input.
pub(crate) async fn load_conversation_attachments(
    ctx: &Ctx,
    urls: Vec<Url>,
) -> Result<Vec<Vec<jp_attachment::Attachment>>> {
    // Handle the missing-conversation case inside each future so the outer
    // `try_join_all` keeps its fail-fast behavior for real errors: a
    // structural failure aborts the batch immediately instead of waiting
    // for slow HTTP/GitHub handlers to finish.
    let futs = urls.into_iter().map(|url| async move {
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
    });
    futures::future::try_join_all(futs).await
}

#[cfg(test)]
#[path = "attachment_tests.rs"]
mod tests;
