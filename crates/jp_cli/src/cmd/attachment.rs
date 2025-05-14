use jp_attachment_bear_note as _;
use jp_attachment_cmd_output as _;
use jp_attachment_file_content as _;
use jp_conversation::Context;
use tracing::{debug, trace};
use url::Url;

use super::Output;
use crate::{
    ctx::Ctx,
    error::{Error, Result},
};

pub(super) mod add;
mod ls;
mod rm;

#[derive(Debug, clap::Args)]
pub struct Args {
    #[command(subcommand)]
    command: Commands,
}

impl Args {
    pub async fn run(self, ctx: &mut Ctx) -> Output {
        match self.command {
            Commands::Add(args) => args.run(ctx).await,
            Commands::Remove(args) => args.run(ctx).await,
            Commands::List(args) => args.run(ctx).await,
        }
    }
}

#[derive(Debug, clap::Subcommand)]
enum Commands {
    /// Add attachment to context.
    #[command(name = "add", alias = "a")]
    Add(add::Args),

    /// Remove attachment from context
    #[command(name = "rm", alias = "r")]
    Remove(rm::Args),

    /// List attachments in context.
    #[command(name = "ls", alias = "l")]
    List(ls::Args),
}

pub async fn register_attachment(uri: &Url, ctx: &mut Context) -> Result<()> {
    trace!(uri = uri.as_str(), "Registering attachment.");

    let scheme = uri.scheme();
    let attachment = if let Some(attachment) = ctx.attachment_handlers.get_mut(scheme) {
        attachment
    } else {
        let Some(handler) = jp_attachment::find_handler_by_scheme(scheme) else {
            return Err(Error::NotFound("Attachment handler", scheme.to_string()));
        };

        ctx.attachment_handlers
            .entry(scheme.to_string())
            .or_insert(handler)
    };

    debug!(%uri, "Registered URI as attachment.");
    attachment
        .add(uri)
        .await
        .map_err(|e| Error::Attachment(e.to_string()))
}

pub async fn unregister_attachment(uri: &str, ctx: &mut Context) -> Result<()> {
    let uri = if let Ok(uri) = Url::parse(uri) {
        uri
    } else {
        // Special case for file attachments
        trace!("URI is not a valid URL, treating as file path.");
        Url::parse(&format!("file:{uri}"))?
    };

    let id = uri.scheme();

    if let Some(attachment) = ctx.attachment_handlers.get_mut(id) {
        attachment
            .remove(&uri)
            .await
            .map_err(|e| Error::Attachment(e.to_string()))?;
    }

    Ok(())
}
