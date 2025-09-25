use jp_config::PartialAppConfig;
use jp_workspace::Workspace;
use url::Url;

use crate::{ctx::Ctx, parser, IntoPartialAppConfig, Output};

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
        partial
            .conversation
            .attachments
            .get_or_insert_default()
            .retain(|v| !self.attachments.contains(v));

        Ok(partial)
    }
}
