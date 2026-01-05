use jp_config::{Config as _, PartialAppConfig, conversation::attachment::AttachmentConfig};
use jp_workspace::Workspace;

use crate::{IntoPartialAppConfig, Output, ctx::Ctx, parser::AttachmentUrlOrPath};

#[derive(Debug, clap::Args)]
pub(crate) struct Rm {
    attachments: Vec<AttachmentUrlOrPath>,
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
        workspace: Option<&Workspace>,
        mut partial: PartialAppConfig,
        _: Option<&PartialAppConfig>,
    ) -> std::result::Result<PartialAppConfig, Box<dyn std::error::Error + Send + Sync>> {
        let mut attachments = vec![];

        let to_remove_attachments = self
            .attachments
            .iter()
            .map(|v| v.parse(workspace.map(Workspace::root)))
            .collect::<Result<Vec<_>, _>>()?;

        for attachment in partial.conversation.attachments {
            let url = AttachmentConfig::from_partial(attachment.clone())?.to_url()?;
            if !to_remove_attachments.contains(&url) {
                attachments.push(attachment);
            }
        }

        partial.conversation.attachments = attachments;
        Ok(partial)
    }
}
