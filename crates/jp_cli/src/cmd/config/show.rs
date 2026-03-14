use jp_config::PartialAppConfig;
use jp_printer::Printer;

use crate::cmd::Output;

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
    pub(crate) fn run_standalone(&self, printer: &Printer) -> Output {
        if self.themes {
            printer.println(list_themes());
            return Ok(());
        }

        // Bare `config show` and `--defaults` both show defaults.
        printer.println(&toml::to_string_pretty(&PartialAppConfig::default())?);
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
