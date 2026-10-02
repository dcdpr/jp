//! Installing plugins from the registry.
//!
//! `jp plugin install <name>` installs a plugin and the plugins it requires,
//! asking first when any of them is third-party.
//! [`install_official`] is the same work for an official command typed before
//! its plugin was installed, which asks nothing.
//!
//! Every binary JP writes is recorded in the approval store as installed:
//! answering the install prompt is the approval to run it, and the record is
//! how JP later tells a binary it may update from one changed on this machine.

use std::collections::HashSet;

use camino::Utf8PathBuf;
use chrono::Utc;
use jp_inquire::{InlineOption, InlineSelect};
use jp_plugin::registry::{ApprovedPlugin, PluginKind, Registry, RegistryPlugin};
use jp_printer::Printer;

use super::{approvals::ApprovalStore, registry};
use crate::cmd;

/// Install a plugin from the registry, with the plugins it requires.
#[derive(Debug, clap::Args)]
pub(crate) struct Install {
    /// Name of the plugin to install (e.g. "serve-web").
    name: String,
}

impl Install {
    pub(crate) async fn run(&self, printer: &Printer, interactive: bool) -> cmd::Output {
        let client = registry::client();
        let reg = registry::fetch_or_load(&client).await?.into_registry();

        let (key, _) = reg
            .plugins
            .iter()
            .find(|(_, plugin)| plugin.id == self.name && plugin.kind.is_command())
            .ok_or_else(|| format!("plugin `{}` not found in registry", self.name))?;

        if let Some(path) = registry::find_installed(&self.name) {
            return Err(format!("plugin `{}` is already installed at {path}", self.name).into());
        }

        let missing: Vec<_> = plan(&reg, key)?
            .into_iter()
            .filter(|(_, plugin)| registry::find_installed(&plugin.id).is_none())
            .collect();

        confirm_third_party(&missing, printer, interactive)?;

        let mut approvals = ApprovalStore::load();
        for (_, plugin) in &missing {
            download(&client, plugin, &mut approvals, printer).await?;
        }

        Ok(())
    }
}

/// Install the official plugin published as `key`, and what it requires.
///
/// For a command typed before its plugin was installed: official plugins behave
/// as part of `jp`, so nothing is asked.
/// Returns the path of the installed binary.
///
/// # Errors
///
/// Fails when the plugin requires a third-party one, which installing it
/// unasked would bring onto the machine, or when a download fails.
pub(crate) async fn install_official(
    reg: &Registry,
    key: &str,
    approvals: &mut ApprovalStore,
    printer: &Printer,
) -> Result<Utf8PathBuf, cmd::Error> {
    let steps = plan(reg, key)?;

    if let Some((dependency, _)) = steps.iter().find(|(_, plugin)| !plugin.official) {
        return Err(format!(
            "the official plugin for `jp {key}` requires the third-party plugin `{dependency}`, \
             which is not installed without asking. Run `jp plugin install` for it first."
        )
        .into());
    }

    let client = registry::client();
    let mut installed = None;

    for (_, plugin) in &steps {
        installed = Some(match registry::find_installed(&plugin.id) {
            Some(path) => path,
            None => download(&client, plugin, approvals, printer).await?,
        });
    }

    installed.ok_or_else(|| format!("`jp {key}` has no binary to install").into())
}

/// The command plugins installing `key` takes, dependencies first.
///
/// Command groups have no binary, so a `requires` naming one needs nothing
/// installed.
///
/// # Errors
///
/// Fails on a requirement the registry does not list, and on a cycle.
pub(crate) fn plan<'a>(
    reg: &'a Registry,
    key: &'a str,
) -> Result<Vec<(&'a str, &'a RegistryPlugin)>, cmd::Error> {
    let mut order = Vec::new();
    let mut visiting = HashSet::new();
    let mut done = HashSet::new();

    visit(reg, key, &mut visiting, &mut done, &mut order)?;
    Ok(order)
}

fn visit<'a>(
    reg: &'a Registry,
    key: &'a str,
    visiting: &mut HashSet<&'a str>,
    done: &mut HashSet<&'a str>,
    order: &mut Vec<(&'a str, &'a RegistryPlugin)>,
) -> Result<(), cmd::Error> {
    if done.contains(key) {
        return Ok(());
    }

    if !visiting.insert(key) {
        return Err(format!("the registry's `requires` for `{key}` form a cycle").into());
    }

    let (key, plugin) = reg
        .plugins
        .get_key_value(key)
        .ok_or_else(|| format!("a plugin requires `{key}`, which the registry does not list"))?;

    if let PluginKind::Command { requires, .. } = &plugin.kind {
        for required in requires {
            visit(reg, required, visiting, done, order)?;
        }

        order.push((key.as_str(), plugin));
    }

    visiting.remove(key.as_str());
    done.insert(key.as_str());
    Ok(())
}

/// Ask once before installing third-party plugins, naming where each comes
/// from.
fn confirm_third_party(
    steps: &[(&str, &RegistryPlugin)],
    printer: &Printer,
    interactive: bool,
) -> Result<(), cmd::Error> {
    let third_party: Vec<_> = steps.iter().filter(|(_, p)| !p.official).collect();
    if third_party.is_empty() {
        return Ok(());
    }

    let names = third_party
        .iter()
        .map(|(_, p)| format!("`{}`", p.id))
        .collect::<Vec<_>>()
        .join(", ");

    if !interactive {
        return Err(format!(
            "installing the third-party plugins {names} needs a terminal to confirm at"
        )
        .into());
    }

    let target = registry::current_target();
    for (_, plugin) in &third_party {
        let from = plugin
            .kind
            .binaries()
            .get(&target)
            .map_or("the registry", |binary| binary.url.as_str());
        printer.prompt_println(format!("  \u{2192} jp-{}, from {from}", plugin.id));
    }

    let question = if third_party.len() == 1 {
        "Install this third-party plugin?"
    } else {
        "Install these third-party plugins?"
    };

    let answer = InlineSelect::new(question, vec![
        InlineOption::new('y', "install"),
        InlineOption::new('n', "cancel"),
    ])
    .with_default('n')
    .prompt(&mut printer.prompt_writer())
    .map_err(|e| cmd::Error::from(format!("prompt failed: {e}")))?;

    if answer != 'y' {
        return Err("installation cancelled".into());
    }

    Ok(())
}

/// Download one plugin's binary for this platform, install it, and record it as
/// installed.
pub(crate) async fn download(
    client: &reqwest::Client,
    plugin: &RegistryPlugin,
    approvals: &mut ApprovalStore,
    printer: &Printer,
) -> Result<Utf8PathBuf, cmd::Error> {
    let target = registry::current_target();
    let binary = registry::release(plugin).ok_or_else(|| {
        format!(
            "no binary available for platform `{target}` (plugin: {})",
            plugin.id
        )
    })?;

    printer.eprintln(format!(
        "  \u{2192} Installing jp-{} for {target}...",
        plugin.id
    ));
    let data = registry::download_and_verify(client, binary).await?;
    let path = registry::install_binary(&plugin.id, &data)?;

    approvals.record(&plugin.id, ApprovedPlugin {
        path: path.clone(),
        sha256: binary.sha256.clone(),
        approved_at: Utc::now(),
        installed: true,
        manifest: None,
    })?;

    printer.eprintln(format!("  \u{2192} Installed to {path}"));
    Ok(path)
}

#[cfg(test)]
#[path = "install_tests.rs"]
mod tests;
