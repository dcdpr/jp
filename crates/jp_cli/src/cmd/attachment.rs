use jp_attachment_bear_note as _;
use jp_attachment_cmd_output as _;
use jp_attachment_file_content as _;
use jp_attachment_http_content as _;
use jp_attachment_internal::{
    resolve as resolve_internal_attachment, validate as validate_internal_attachment,
};
use jp_attachment_mcp_resources as _;
use jp_config::PartialAppConfig;
use jp_workspace::Workspace;
use tracing::trace;
use url::Url;

use super::Output;
use crate::{
    IntoPartialAppConfig,
    ctx::Ctx,
    error::{Error, Result},
};

pub(super) mod add;
mod ls;
mod print;
mod rm;

#[derive(Debug, clap::Args)]
pub(crate) struct Attachment {
    #[command(subcommand)]
    command: Commands,
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

#[derive(Debug, clap::Subcommand)]
enum Commands {
    /// Add attachment to context.
    #[command(name = "add", alias = "a")]
    Add(add::Add),

    /// Remove attachment from context
    #[command(name = "rm", alias = "r")]
    Remove(rm::Rm),

    /// List attachments in context.
    #[command(name = "ls", alias = "l")]
    List(ls::Ls),

    /// Preview how an attachment will render for the LLM.
    #[command(name = "print", alias = "p")]
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
        validate_internal_attachment(uri).map_err(|e| Error::Attachment(e.to_string()))?;
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
        return resolve_internal_attachment(&ctx.workspace, &uri)
            .map_err(|e| Error::Attachment(e.to_string()));
    }

    let Some(mut handler) = jp_attachment::find_handler_by_scheme(scheme) else {
        return Err(Error::NotFound("Attachment handler", scheme.to_string()));
    };

    handler
        .add(&uri, ctx.workspace.root())
        .await
        .map_err(|e| Error::Attachment(e.to_string()))?;

    handler
        .get(ctx.workspace.root(), ctx.mcp_client.clone())
        .await
        .map_err(|e| Error::Attachment(e.to_string()))
}
