//! `jp plugin install` subcommand.

use std::io::Write as _;

use jp_inquire::{InlineOption, InlineSelect};

use super::registry;
use crate::{Ctx, cmd};

/// Install a plugin from the registry.
#[derive(Debug, clap::Args)]
pub(crate) struct Install {
    /// Name of the plugin to install (e.g. "serve").
    name: String,
}

impl Install {
    pub(crate) async fn run(&self, ctx: &Ctx) -> cmd::Output {
        // Check if already installed.
        if let Some(path) = registry::find_installed(&self.name) {
            return Err(cmd::Error::from(format!(
                "plugin `{}` is already installed at {path}",
                self.name,
            )));
        }

        let client = reqwest::Client::new();
        let reg = registry::fetch_or_load(&client).await?;

        // Look up by id (the stable identifier used for binary naming).
        let plugin = reg
            .plugins
            .values()
            .find(|p| p.id == self.name)
            .ok_or_else(|| {
                cmd::Error::from(format!("plugin `{}` not found in registry", self.name))
            })?;

        let target = registry::current_target();
        let binary = plugin.kind.binaries().get(&target).ok_or_else(|| {
            cmd::Error::from(format!(
                "no binary available for platform `{target}` (plugin: {})",
                self.name
            ))
        })?;

        let mut err = std::io::stderr();

        // Third-party plugins need explicit approval.
        if !plugin.official {
            if !ctx.term.is_tty {
                return Err(cmd::Error::from(format!(
                    "plugin `{}` is third-party and requires interactive approval",
                    self.name
                )));
            }

            drop(writeln!(
                err,
                "  \u{2192} Plugin `{}` is third-party (not official).",
                self.name
            ));
            let options = vec![
                InlineOption::new('y', "install"),
                InlineOption::new('n', "cancel"),
            ];
            let answer = InlineSelect::new("Install it?", options)
                .prompt(&mut err)
                .map_err(|e| cmd::Error::from(format!("prompt failed: {e}")))?;
            if answer != 'y' {
                return Err(cmd::Error::from("installation cancelled"));
            }
        }

        drop(writeln!(
            err,
            "  \u{2192} Downloading jp-{} for {target}...",
            self.name
        ));
        let data = registry::download_and_verify(&client, binary).await?;

        let path = registry::install_binary(&self.name, &data)?;
        drop(writeln!(err, "  \u{2192} Installed to {path}"));

        Ok(())
    }
}
