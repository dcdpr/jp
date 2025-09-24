use jp_config::PartialAppConfig;

use crate::{ctx::Ctx, Output};

#[derive(Debug, clap::Args)]
pub(crate) struct Show {
    /// Show the default configurations.
    #[arg(long)]
    defaults: bool,
}

impl Show {
    pub(crate) fn run(self, _ctx: &mut Ctx) -> Output {
        if self.defaults {
            return Ok(toml::to_string_pretty(&PartialAppConfig::default())?.into());
        }

        Ok(().into())
    }
}
