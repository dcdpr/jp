use jp_config::{Config as _, PartialAppConfig, conversation::attachment::AttachmentConfig};
use jp_workspace::Workspace;
use url::Url;

use crate::{IntoPartialAppConfig, Output, ctx::Ctx, parser};

#[derive(Debug, clap::Args)]
pub(crate) struct Rm {
    #[arg(value_parser = parser::attachment_url)]
    attachments: Vec<Url>,
}

impl Rm {
    #[expect(clippy::unused_self, clippy::unnecessary_wraps)]
    pub(crate) fn run(self, _: &mut Ctx) -> Output {
        // See `apply_cli_config` for implementation.

        Ok(().into())
    }
}

impl IntoPartialAppConfig for Rm {
    fn apply_cli_config(
        &self,
        _: Option<&Workspace>,
        mut partial: PartialAppConfig,
        _: Option<&PartialAppConfig>,
    ) -> std::result::Result<PartialAppConfig, Box<dyn std::error::Error + Send + Sync>> {
        let mut attachments = vec![];
        for attachment in partial.conversation.attachments {
            let url = AttachmentConfig::from_partial(attachment.clone())?.to_url()?;
            if !self.attachments.contains(&url) {
                attachments.push(attachment);
            }
        }

        partial.conversation.attachments = attachments;
        Ok(partial)
    }
}
