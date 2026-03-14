use std::collections::BTreeMap;

use jp_config::AppConfig;
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
        printer.println(config_skeleton());
        Ok(())
    }
}

/// Build a commented TOML skeleton showing all available config keys.
fn config_skeleton() -> String {
    let fields = AppConfig::fields();

    // Group fields by their TOML section path (everything before the last
    // dot-separated component). Top-level fields have an empty section key.
    let mut sections: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for field in &fields {
        let (section, key) = match field.rsplit_once('.') {
            Some((s, k)) => (s.to_owned(), k.to_owned()),
            None => (String::new(), field.clone()),
        };
        sections.entry(section).or_default().push(key);
    }

    let mut out = String::new();

    for (section, keys) in &sections {
        if !out.is_empty() {
            out.push('\n');
        }

        if !section.is_empty() {
            out.push_str(&format!("[{section}]\n"));
        }

        for key in keys {
            out.push_str(&format!("# {key} =\n"));
        }
    }

    out
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
