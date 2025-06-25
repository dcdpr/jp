use jp_config::PartialConfig;
use jp_workspace::Workspace;
use url::Url;

use crate::{ctx::Ctx, parser, IntoPartialConfig, Output};

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

impl IntoPartialConfig for Rm {
    fn apply_cli_config(
        &self,
        _: Option<&Workspace>,
        mut partial: PartialConfig,
    ) -> std::result::Result<PartialConfig, Box<dyn std::error::Error + Send + Sync>> {
        partial
            .conversation
            .attachments
            .get_or_insert_default()
            .retain(|v| !self.attachments.contains(v));

        Ok(partial)
    }
}
