//! `jp plugin approve` subcommand.

use camino::{Utf8Path, Utf8PathBuf, absolute_utf8};
use chrono::Utc;
use jp_config::{PartialAppConfig, plugins::command::RunPolicy};
use jp_plugin::{PROTOCOL_VERSION, registry::ApprovedPlugin};
use jp_printer::Printer;

use super::{
    admission::pin_mismatch,
    approvals::ApprovalStore,
    discovery::{self, LocalPlugin, Location, ManifestState},
    dispatch, registry,
};
use crate::{KeyValueOrPath, cmd, load_user_global_partial};

/// Approve a plugin binary, so it runs without asking.
///
/// Naming the file is the consent to run it once: JP asks it to describe
/// itself, checks the answer against the binary's manifest, and remembers this
/// file with these contents.
#[derive(Debug, clap::Args)]
pub(crate) struct Approve {
    /// Path to the plugin binary, a file named `jp-<name>`.
    path: Utf8PathBuf,
}

impl Approve {
    pub(crate) fn run(&self, printer: &Printer, cfg: &[KeyValueOrPath]) -> cmd::Output {
        // Named by the file given, the way discovery names it: a link on
        // `$PATH` into a versioned install is the plugin its link name says,
        // whatever the file it points at is called.
        let name = self
            .path
            .file_name()
            .and_then(discovery::plugin_name)
            .ok_or_else(|| {
                format!(
                    "{} is not a plugin binary: its name has to start with `jp-`",
                    self.path
                )
            })?
            .to_owned();

        let canonical = self
            .path
            .canonicalize_utf8()
            .map_err(|e| format!("cannot find a plugin binary at {}: {e}", self.path))?;
        let path = absolute_utf8(&self.path)
            .map_err(|e| format!("cannot resolve the path {}: {e}", self.path))?;

        // Approving runs the plugin, so what configuration refuses holds here
        // too. Outside any workspace, the configuration is the user-global one
        // with `--cfg` on top.
        let config = load_user_global_partial(cfg)?;
        check_not_denied(&config, &name)?;

        // Hashed before the plugin runs, so the approval is for the contents
        // that were checked: a binary that changes itself while answering is
        // asked about again on its next run.
        let sha256 = registry::sha256_file(&canonical)?;
        check_pin(&config, &name, &path, &sha256)?;

        let location = if registry::bin_dir()
            .and_then(|dir| dir.canonicalize_utf8().ok())
            .is_some_and(|dir| canonical.starts_with(dir))
        {
            Location::InstallDir
        } else {
            Location::Path
        };

        let plugin = LocalPlugin {
            manifest: discovery::read_manifest(&canonical),
            name,
            path,
            location,
        };

        let recorded = approval_manifest(&plugin)?;
        let answer = dispatch::describe(&plugin)?;

        if answer.manifest.protocol > PROTOCOL_VERSION {
            return Err(format!(
                "plugin `{}` needs protocol {}, and this `jp` speaks {PROTOCOL_VERSION}",
                plugin.name, answer.manifest.protocol,
            )
            .into());
        }

        ApprovalStore::load().record(&plugin.name, ApprovedPlugin {
            path: plugin.path.clone(),
            sha256,
            approved_at: Utc::now(),
            installed: false,
            manifest: recorded.then(|| answer.manifest.clone()),
        })?;

        printer.eprintln(format!(
            "  \u{2192} Approved `{}`: `jp {}`, {}",
            plugin.name,
            answer.manifest.command.join(" "),
            answer.manifest.description,
        ));
        printer.eprintln(format!("    {}", plugin.path));

        Ok(())
    }
}

/// Refuse to run a plugin to approve it when `config` denies it.
fn check_not_denied(config: &PartialAppConfig, name: &str) -> cmd::Output {
    if config.plugins.command.get(name).and_then(|c| c.run) == Some(RunPolicy::Deny) {
        return Err(format!(
            "plugin `{name}` is denied by configuration (plugins.command.{name}.run = \"deny\"), \
             so it is not run to approve it"
        )
        .into());
    }

    Ok(())
}

/// Refuse to run a plugin to approve it when `config` pins its checksum to
/// other contents than `sha256`.
///
/// The approval would be useless as well: admission checks the pin before it
/// looks at approvals.
fn check_pin(config: &PartialAppConfig, name: &str, path: &Utf8Path, sha256: &str) -> cmd::Output {
    let pinned = config
        .plugins
        .command
        .get(name)
        .and_then(|c| c.checksum.as_ref())
        .and_then(|c| c.value.as_deref());

    match pinned.and_then(|pinned| pin_mismatch(name, path, pinned, sha256)) {
        Some(refusal) => Err(refusal.into()),
        None => Ok(()),
    }
}

/// Whether the approval records the manifest from the plugin's answer.
///
/// Only for a binary whose file carries none, such as one compressed with UPX:
/// the answer is then the only place its claims come from.
/// A manifest that is present but unusable is refused rather than replaced.
fn approval_manifest(plugin: &LocalPlugin) -> Result<bool, cmd::Error> {
    match &plugin.manifest {
        ManifestState::Valid(_) | ManifestState::Recorded(_) => Ok(false),
        ManifestState::Missing => Ok(true),
        ManifestState::Invalid(reason) => Err(format!(
            "plugin `{}` at {} carries an invalid manifest ({reason}), and is not approved",
            plugin.name, plugin.path,
        )
        .into()),
    }
}

#[cfg(test)]
#[path = "approve_tests.rs"]
mod tests;
