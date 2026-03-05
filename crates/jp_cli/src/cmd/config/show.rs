use jp_config::PartialAppConfig;

use crate::{cmd::Output, ctx::Ctx};

#[derive(Debug, clap::Args)]
pub(crate) struct Show {
    /// Show the default configurations.
    #[arg(long)]
    defaults: bool,

    /// List available syntax highlighting themes.
    #[arg(long)]
    themes: bool,
}

impl Show {
    pub(crate) fn run(self, ctx: &mut Ctx) -> Output {
        if self.defaults {
            ctx.printer
                .println(&toml::to_string_pretty(&PartialAppConfig::default())?);
            return Ok(());
        }

        if self.themes {
            ctx.printer.println(list_themes());
            return Ok(());
        }

        Ok(())
    }
}

/// Build a human-readable list of available syntax highlighting themes.
fn list_themes() -> String {
    let default_name = jp_md::theme::default_theme_name();

    jp_md::theme::all_theme_names()
        .iter()
        .map(|t| {
            let name = t.as_name();
            if name == default_name {
                format!("{name} (default)")
            } else {
                name.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}
