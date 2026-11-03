use std::collections::BTreeMap;

use clap::{Arg, ArgAction, ArgMatches, Command};
use jp_config::{AppConfig, PartialAppConfig};
use schematic::PartialConfig as _;

use crate::{Ctx, cli::CommandArgs, cmd::Output};

/// `jp config show`.
#[derive(Debug)]
pub(crate) struct Show {
    /// Print the default configuration instead.
    defaults: bool,

    /// List the syntax highlighting themes instead.
    themes: bool,
}

impl CommandArgs for Show {
    fn augment(cmd: Command) -> Command {
        cmd.arg(
            Arg::new("defaults")
                .long("defaults")
                .action(ArgAction::SetTrue)
                .help("Show the default configurations"),
        )
        .arg(
            Arg::new("themes")
                .long("themes")
                .action(ArgAction::SetTrue)
                .help("List available syntax highlighting themes"),
        )
    }

    fn from_matches(matches: &ArgMatches) -> Result<Self, clap::Error> {
        Ok(Self {
            defaults: matches.get_flag("defaults"),
            themes: matches.get_flag("themes"),
        })
    }
}

impl Show {
    pub(crate) fn run(self, ctx: &mut Ctx) -> Output {
        if self.defaults {
            ctx.printer
                .println(toml::to_string_pretty(&PartialAppConfig::default_values(
                    &(),
                )?)?);
            return Ok(());
        }

        if self.themes {
            ctx.printer.println(list_themes());
            return Ok(());
        }

        ctx.printer.println(config_skeleton());
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
