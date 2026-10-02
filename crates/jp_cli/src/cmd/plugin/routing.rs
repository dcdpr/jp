//! Choosing the plugin an unknown subcommand goes to.
//!
//! [`route`] is pure: it takes the arguments, the plugin binaries on this
//! machine, and the cached registry, and decides which of them the command
//! belongs to.
//! Built-in commands never reach it, because clap matches them first.
//!
//! Several sources can claim one command path, and the rules that decide
//! between them are in `docs/rfd/072-command-plugin-system.md`, Phase 5:
//!
//! 1. A built-in command wins (clap's job, before this runs).
//! 2. The longest claimed path the arguments start with wins.
//! 3. A third-party binary claiming an official command wins over the official
//!    plugin.
//! 4. Two third-party binaries claiming one path are an error naming both.
//! 5. An official binary must claim the path its registry key names.
//! 6. Two binaries with the same name are an error naming both.

use std::collections::BTreeMap;

use camino::Utf8PathBuf;
use jp_plugin::registry::{PluginKind, Registry, RegistryPlugin};

use super::discovery::LocalPlugin;

/// Where an unknown subcommand goes.
#[derive(Debug, PartialEq)]
pub(crate) enum Route<'a> {
    /// A binary on this machine handles it.
    Local {
        /// The binary.
        plugin: &'a LocalPlugin,

        /// How many arguments name the command; the rest go to the plugin.
        consumed: usize,

        /// The registry key of the official command a third-party binary
        /// replaces (rule 3).
        replaces: Option<&'a str>,
    },

    /// An official plugin handles it.
    Official {
        /// The registry key, the command path it claims.
        key: &'a str,

        /// The registry entry.
        entry: &'a RegistryPlugin,

        /// The installed binary, or `None` when it has to be downloaded.
        binary: Option<&'a LocalPlugin>,

        /// How many arguments name the command.
        consumed: usize,
    },

    /// An official command group, which has no binary of its own.
    Group {
        /// The registry key.
        key: &'a str,

        /// The registry entry.
        entry: &'a RegistryPlugin,

        /// How many arguments name the group.
        consumed: usize,
    },

    /// Nothing claims the command, but a third-party registry entry names it.
    ThirdParty {
        /// The registry key.
        key: &'a str,

        /// The registry entry, whose `id` is what `jp plugin install` takes.
        entry: &'a RegistryPlugin,
    },

    /// Nothing claims the command.
    NotFound,
}

/// Why a command cannot be routed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum RouteError {
    /// Two binaries share a name, and with it a configuration and an approval
    /// (rule 6).
    #[error(
        "two plugins are named `{name}`:\n{}\nThey would share one configuration and one \
         approval, so neither runs. Rename one: the command it handles comes from its manifest, \
         not its file name.",
        list(paths)
    )]
    SameName {
        /// The shared name.
        name: String,

        /// Every binary with that name.
        paths: Vec<Utf8PathBuf>,
    },

    /// Two third-party binaries claim one command path (rule 4).
    #[error(
        "`jp {command}` is claimed by more than one plugin:\n{}\nRemove or rename all but one.",
        list(paths)
    )]
    Ambiguous {
        /// The claimed command path, space-separated.
        command: String,

        /// Every binary claiming it.
        paths: Vec<Utf8PathBuf>,
    },

    /// An official binary claims a path other than its registry key (rule 5).
    #[error(
        "the official plugin `{name}` at {path} claims `jp {claimed}`, but the registry publishes \
         it as `jp {expected}`. Remove it (`jp plugin uninstall {name}` removes a copy JP \
         installed), and the next `jp {expected}` installs the current release."
    )]
    OfficialMismatch {
        /// The plugin's name, its registry `id`.
        name: String,

        /// Where the binary is.
        path: Utf8PathBuf,

        /// What its manifest claims.
        claimed: String,

        /// What its registry key names.
        expected: String,
    },

    /// An official binary carries no usable manifest.
    #[error(
        "the official plugin `{name}` at {path} cannot be used: {reason}. Remove it (`jp plugin \
         uninstall {name}` removes a copy JP installed), and the next `jp {command}` installs the \
         current release."
    )]
    InvalidManifest {
        /// The plugin's name, its registry `id`.
        name: String,

        /// Where the binary is.
        path: Utf8PathBuf,

        /// Why its manifest cannot be used.
        reason: String,

        /// The command path its registry key names.
        command: String,
    },
}

fn list(paths: &[Utf8PathBuf]) -> String {
    paths
        .iter()
        .map(|path| format!("  {path}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Decide where `args` goes.
///
/// `args` starts with the unknown subcommand.
/// `registry` is the cached registry, if there is one; without it, only the
/// binaries on this machine claim anything.
///
/// # Errors
///
/// Returns a [`RouteError`] when the claims for the chosen command conflict.
pub(crate) fn route<'a>(
    args: &[String],
    local: &'a [LocalPlugin],
    registry: Option<&'a Registry>,
) -> Result<Route<'a>, RouteError> {
    let official = official_entries(registry);

    // The longest claimed path the arguments start with (rule 2).
    let local_claims = local
        .iter()
        .filter_map(|plugin| plugin.manifest.valid())
        .filter(|manifest| is_prefix(&manifest.command, args))
        .map(|manifest| manifest.command.len());
    let official_claims = official
        .keys()
        .map(|key| segments(key))
        .filter(|path| is_prefix(path, args))
        .map(|path| path.len());

    let Some(consumed) = local_claims.chain(official_claims).max() else {
        return Ok(third_party_hint(args, registry));
    };

    let path = &args[..consumed];
    let official_here = official
        .iter()
        .find(|(key, _)| segments(key) == path)
        .map(|(key, entry)| (*key, *entry));

    // The key an official binary is published under, and `None` for a
    // third-party one.
    let published_as = |plugin: &LocalPlugin| {
        official
            .iter()
            .find(|(_, entry)| entry.kind.is_command() && entry.id == plugin.name)
            .map(|(key, _)| *key)
    };

    // Rule 3: a third-party binary wins over the official plugin.
    let third_party: Vec<&LocalPlugin> = local
        .iter()
        .filter(|p| claims_path(p, path) && published_as(p).is_none())
        .collect();

    match third_party.as_slice() {
        [] => {}
        [plugin] => {
            check_unique_name(local, &plugin.name)?;
            return Ok(Route::Local {
                plugin,
                consumed,
                replaces: official_here.map(|(key, _)| key),
            });
        }
        many => {
            return Err(RouteError::Ambiguous {
                command: path.join(" "),
                paths: many.iter().map(|p| p.path.clone()).collect(),
            });
        }
    }

    // An official binary whose manifest claims this path, when the registry
    // publishes it elsewhere (rule 5).
    for plugin in local.iter().filter(|p| claims_path(p, path)) {
        if let Some(key) = published_as(plugin)
            && segments(key) != path
        {
            return Err(RouteError::OfficialMismatch {
                name: plugin.name.clone(),
                path: plugin.path.clone(),
                claimed: path.join(" "),
                expected: key.to_owned(),
            });
        }
    }

    let Some((key, entry)) = official_here else {
        unreachable!("the longest match came from a local or an official claim");
    };

    if entry.kind.is_command_group() {
        return Ok(Route::Group {
            key,
            entry,
            consumed,
        });
    }

    check_unique_name(local, &entry.id)?;
    let binary = local.iter().find(|p| p.name == entry.id);

    // The official binary's own manifest has to agree with the registry.
    if let Some(plugin) = binary {
        match plugin.manifest.valid() {
            Some(manifest) if manifest.command != path => {
                return Err(RouteError::OfficialMismatch {
                    name: plugin.name.clone(),
                    path: plugin.path.clone(),
                    claimed: manifest.command.join(" "),
                    expected: key.to_owned(),
                });
            }
            Some(_) => {}
            None => {
                return Err(RouteError::InvalidManifest {
                    name: plugin.name.clone(),
                    path: plugin.path.clone(),
                    reason: plugin.manifest.problem().unwrap_or_default(),
                    command: key.to_owned(),
                });
            }
        }
    }

    Ok(Route::Official {
        key,
        entry,
        binary,
        consumed,
    })
}

/// A plugin found by its name, for a tool whose source names it.
#[derive(Debug, PartialEq)]
pub(crate) struct Named<'a> {
    /// The binary.
    pub plugin: &'a LocalPlugin,

    /// The official registry entry whose `id` is the plugin's name, when it is
    /// one.
    pub official: Option<&'a RegistryPlugin>,
}

/// Find the plugin named `name`, the way a tool's `command.<name>` source names
/// it.
///
/// A name, not a command path: a tool reaches a plugin by its identity, which
/// keys its configuration and its approval, whatever command it claims.
/// Returns `None` when no binary on this machine has that name.
///
/// # Errors
///
/// [`RouteError::SameName`] when two binaries share it.
pub(crate) fn by_name<'a>(
    name: &str,
    local: &'a [LocalPlugin],
    registry: Option<&'a Registry>,
) -> Result<Option<Named<'a>>, RouteError> {
    check_unique_name(local, name)?;

    let Some(plugin) = local.iter().find(|plugin| plugin.name == name) else {
        return Ok(None);
    };

    let official = official_entries(registry)
        .into_values()
        .find(|entry| entry.kind.is_command() && entry.id == name);

    Ok(Some(Named { plugin, official }))
}

/// The registry's official entries, by key.
///
/// Only these claim commands; a third-party entry is a catalog entry.
pub(crate) fn official_entries(registry: Option<&Registry>) -> BTreeMap<&str, &RegistryPlugin> {
    registry
        .into_iter()
        .flat_map(|registry| registry.plugins.iter())
        .filter(|(_, entry)| entry.official && is_routable(&entry.kind))
        .map(|(key, entry)| (key.as_str(), entry))
        .collect()
}

/// Whether this host knows how to route to an entry of this kind.
fn is_routable(kind: &PluginKind) -> bool {
    kind.is_command() || kind.is_command_group()
}

/// A registry key's command path.
pub(crate) fn segments(key: &str) -> Vec<&str> {
    key.split(' ').collect()
}

/// Whether `args` starts with the command path `claim`.
fn is_prefix<S: AsRef<str>>(claim: &[S], args: &[String]) -> bool {
    claim.len() <= args.len()
        && claim
            .iter()
            .zip(args)
            .all(|(segment, arg)| segment.as_ref() == arg)
}

/// Whether `plugin`'s manifest claims exactly `path`.
fn claims_path(plugin: &LocalPlugin, path: &[String]) -> bool {
    plugin
        .manifest
        .valid()
        .is_some_and(|manifest| manifest.command == path)
}

/// Refuse a name two binaries share (rule 6).
fn check_unique_name(local: &[LocalPlugin], name: &str) -> Result<(), RouteError> {
    let paths: Vec<Utf8PathBuf> = local
        .iter()
        .filter(|p| p.name == name)
        .map(|p| p.path.clone())
        .collect();

    if paths.len() > 1 {
        return Err(RouteError::SameName {
            name: name.to_owned(),
            paths,
        });
    }

    Ok(())
}

/// The third-party registry entry with the longest key `args` starts with.
///
/// A catalog entry, for the hint an unknown command's error gives; it never
/// decides which plugin runs.
pub(crate) fn third_party_hint<'a>(args: &[String], registry: Option<&'a Registry>) -> Route<'a> {
    registry
        .into_iter()
        .flat_map(|registry| registry.plugins.iter())
        .filter(|(_, entry)| !entry.official && entry.kind.is_command())
        .filter(|(key, _)| is_prefix(&segments(key), args))
        .max_by_key(|(key, _)| segments(key).len())
        .map_or(Route::NotFound, |(key, entry)| Route::ThirdParty {
            key: key.as_str(),
            entry,
        })
}

#[cfg(test)]
#[path = "routing_tests.rs"]
mod tests;
