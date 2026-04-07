//! Generate `plugins.json` from workspace metadata and command groups.
//!
//! Reads `[package.metadata.jp-registry]` from all workspace crates and merges
//! with an optional groups file and checksums file to produce a `Registry` JSON
//! document on stdout.
//!
//! Usage: `build-registry --help`

#![allow(clippy::print_stderr, clippy::print_stdout)]

use std::{
    collections::BTreeMap,
    fs,
    io::{self, BufRead},
    process::ExitCode,
};

use clap::Parser;
use jp_plugin::registry::{PluginKind, Registry, RegistryPlugin};

/// Metadata from `[package.metadata.jp-registry]` in a plugin's Cargo.toml.
#[derive(serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
struct PluginMeta {
    id: String,
    command: Vec<String>,
    description: String,
    #[serde(default)]
    official: bool,
    #[serde(default)]
    requires: Vec<String>,
    #[serde(default)]
    suggests: Vec<String>,
    repository: Option<String>,
}

/// The top-level structure of the groups TOML file.
#[derive(serde::Deserialize)]
struct GroupsFile {
    group: Vec<GroupEntry>,
}

/// An entry in the groups TOML file.
#[derive(serde::Deserialize)]
struct GroupEntry {
    id: String,
    command: Vec<String>,
    description: String,
    #[serde(default)]
    official: bool,
    #[serde(default)]
    suggests: Vec<String>,
}

/// A parsed checksums line: `<plugin-id> <target-triple> <sha256>`.
struct Checksum {
    id: String,
    target: String,
    sha256: String,
}

#[derive(Parser)]
#[command(about = "Generate plugins.json from workspace metadata and command groups.")]
struct Args {
    /// Path to command groups TOML file.
    #[arg(long)]
    groups: Option<String>,

    /// Path to checksums file (id target sha256).
    #[arg(long)]
    checksums: Option<String>,

    /// Base URL for binary downloads.
    #[arg(long)]
    release_url: Option<String>,
}

fn main() -> ExitCode {
    if let Err(error) = run() {
        eprintln!("{error}");
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

fn run() -> Result<(), String> {
    let args = Args::parse();

    let mut registry = Registry {
        version: 1,
        plugins: BTreeMap::new(),
    };

    // Load command groups.
    if let Some(path) = &args.groups {
        for group in load_groups(path)? {
            let key = group.command.join(" ");
            registry.plugins.insert(key, RegistryPlugin {
                id: group.id,
                description: group.description,
                official: group.official,
                repository: None,
                kind: PluginKind::CommandGroup {
                    suggests: group.suggests,
                },
            });
        }
    }

    // Read plugin metadata from the workspace.
    let metadata = cargo_metadata::MetadataCommand::new()
        .no_deps()
        .exec()
        .map_err(|e| format!("cargo metadata failed: {e}"))?;

    for pkg in metadata.workspace_packages() {
        let Some(meta) = pkg.metadata.get("jp-registry") else {
            continue;
        };

        let plugin_meta: PluginMeta = serde_json::from_value(meta.clone())
            .map_err(|e| format!("invalid jp-registry metadata in {}: {e}", pkg.name))?;

        let key = plugin_meta.command.join(" ");
        registry.plugins.insert(key, RegistryPlugin {
            id: plugin_meta.id,
            description: plugin_meta.description,
            official: plugin_meta.official,
            repository: plugin_meta.repository,
            kind: PluginKind::Command {
                requires: plugin_meta.requires,
                suggests: plugin_meta.suggests,
                binaries: BTreeMap::new(),
            },
        });
    }

    // Apply checksums if provided.
    if let Some(path) = &args.checksums {
        let checksums = load_checksums(path)?;
        let release_url = args
            .release_url
            .as_deref()
            .unwrap_or("https://github.com/dcdpr/jp/releases/download/plugins");

        for cs in checksums {
            // Find the registry entry whose id matches.
            let entry = registry
                .plugins
                .values_mut()
                .find(|p| p.id == cs.id)
                .ok_or_else(|| format!("checksum references unknown plugin id: {}", cs.id))?;

            let binaries = entry
                .kind
                .binaries_mut()
                .ok_or_else(|| format!("checksum for `{}` but plugin is not a command", cs.id))?;

            let url = format!("{release_url}/jp-{}-{}", cs.id, cs.target);
            binaries.insert(cs.target, jp_plugin::registry::RegistryBinary {
                url,
                sha256: cs.sha256,
            });
        }
    }

    let json = serde_json::to_string_pretty(&registry)
        .map_err(|e| format!("failed to serialize registry: {e}"))?;

    println!("{json}");
    Ok(())
}

fn load_groups(path: &str) -> Result<Vec<GroupEntry>, String> {
    let content =
        fs::read_to_string(path).map_err(|e| format!("failed to read groups file {path}: {e}"))?;
    let file: GroupsFile =
        toml::from_str(&content).map_err(|e| format!("invalid groups TOML in {path}: {e}"))?;

    Ok(file.group)
}

fn load_checksums(path: &str) -> Result<Vec<Checksum>, String> {
    let file =
        fs::File::open(path).map_err(|e| format!("failed to open checksums file {path}: {e}"))?;

    io::BufReader::new(file)
        .lines()
        .enumerate()
        .filter_map(|(i, line)| {
            let line = line
                .map_err(|e| format!("failed to read line {} of {path}: {e}", i + 1))
                .ok()?;
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return None;
            }
            Some(parse_checksum_line(trimmed, i + 1, path))
        })
        .collect()
}

fn parse_checksum_line(line: &str, line_num: usize, path: &str) -> Result<Checksum, String> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() != 3 {
        return Err(format!(
            "{path}:{line_num}: expected 3 fields (id target sha256), got {}",
            parts.len()
        ));
    }
    Ok(Checksum {
        id: parts[0].to_owned(),
        target: parts[1].to_owned(),
        sha256: parts[2].to_owned(),
    })
}
