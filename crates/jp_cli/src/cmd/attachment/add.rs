use jp_config::PartialAppConfig;
use jp_workspace::Workspace;
use url::Url;

use super::validate_attachment;
use crate::{ctx::Ctx, parser, IntoPartialAppConfig, Output};

#[derive(Debug, clap::Args)]
#[command(arg_required_else_help(true))]
pub(crate) struct Add {
    /// One or more attachments to add to the context.
    ///
    /// If the attachment is pointing to a file, then a file attachment is
    /// added, otherwise the attachment type can be added as a prefix.
    ///
    /// For example, to add a `summary` attachment, use `summary://<path>`.
    #[arg(value_parser = parser::attachment_url)]
    attachments: Vec<Url>,
}

impl Add {
    #[expect(clippy::unused_self, clippy::unnecessary_wraps)]
    pub(crate) fn run(self, _: &mut Ctx) -> Output {
        // See `apply_cli_config` for implementation.

        Ok(().into())
    }
}

impl IntoPartialAppConfig for Add {
    fn apply_cli_config(
        &self,
        _: Option<&Workspace>,
        mut partial: PartialAppConfig,
        _: Option<&PartialAppConfig>,
    ) -> std::result::Result<PartialAppConfig, Box<dyn std::error::Error + Send + Sync>> {
        for uri in &self.attachments {
            validate_attachment(uri)?;

            partial
                .conversation
                .attachments
                .get_or_insert_default()
                .push(uri.clone());
        }

        Ok(partial)
    }
}
