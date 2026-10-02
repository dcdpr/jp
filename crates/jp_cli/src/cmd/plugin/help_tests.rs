use std::collections::BTreeMap;

use camino::Utf8PathBuf;
use jp_plugin::{
    Manifest,
    registry::{PluginKind, RegistryPlugin},
};
use pretty_assertions::assert_eq;

use super::*;
use crate::cmd::plugin::discovery::{Location, ManifestState};

fn plugin(name: &str, command: &[&str], description: &str) -> LocalPlugin {
    LocalPlugin {
        name: name.to_owned(),
        path: Utf8PathBuf::from(format!("/bin/jp-{name}")),
        location: Location::Path,
        manifest: ManifestState::Valid(Manifest {
            protocol: 1,
            description: description.to_owned(),
            command: command.iter().map(|s| (*s).to_owned()).collect(),
        }),
    }
}

fn entry(id: &str, official: bool, description: &str) -> RegistryPlugin {
    RegistryPlugin {
        id: id.to_owned(),
        description: description.to_owned(),
        official,
        repository: None,
        kind: PluginKind::default(),
    }
}

fn registry() -> Registry {
    Registry {
        version: 1,
        plugins: BTreeMap::from([
            ("serve".to_owned(), RegistryPlugin {
                kind: PluginKind::CommandGroup {
                    suggests: vec!["serve web".to_owned()],
                },
                ..entry("serve", true, "JP server components")
            }),
            (
                "serve web".to_owned(),
                entry("serve-web", true, "Web UI for conversations"),
            ),
            (
                "serve http-api".to_owned(),
                entry("serve-http-api", true, "HTTP API for conversations"),
            ),
            (
                "serve metrics".to_owned(),
                entry("metrics", false, "Prometheus exporter"),
            ),
            (
                "path".to_owned(),
                entry("path", true, "Print JP directory paths"),
            ),
        ]),
    }
}

fn names(entries: &[Entry]) -> Vec<(&str, &str)> {
    entries
        .iter()
        .map(|e| (e.name.as_str(), e.description.as_str()))
        .collect()
}

#[test]
fn root_help_lists_manifests_and_official_commands_not_yet_installed() {
    let registry = registry();
    let local = [
        plugin("ticket", &["ticket"], "Track work items"),
        plugin("serve-web", &["serve", "web"], "Web UI, installed"),
    ];

    assert_eq!(names(&root_entries(&local, Some(&registry))), [
        ("path", "Print JP directory paths"),
        ("serve http-api", "HTTP API for conversations"),
        ("serve web", "Web UI, installed"),
        ("ticket", "Track work items"),
    ]);
}

#[test]
fn root_help_lists_a_binary_without_a_manifest_by_file_name() {
    let local = [
        LocalPlugin {
            manifest: ManifestState::Missing,
            ..plugin("tools", &["unused"], "")
        },
        LocalPlugin {
            manifest: ManifestState::Invalid("too long".to_owned()),
            ..plugin("broken", &["unused"], "")
        },
    ];

    assert_eq!(names(&root_entries(&local, None)), [
        ("broken", "(invalid manifest: too long)"),
        ("tools", "(no plugin manifest)"),
    ]);
}

#[test]
fn a_group_lists_what_is_installed_published_and_third_party() {
    let registry = registry();
    let local = [plugin("serve-web", &["serve", "web"], "Web UI, installed")];

    assert_eq!(
        names(&children(&["serve".to_owned()], &local, Some(&registry))),
        [
            ("http-api", "HTTP API for conversations (not installed)"),
            (
                "metrics",
                "Prometheus exporter (third-party: `jp plugin install metrics`)"
            ),
            ("web", "Web UI, installed"),
        ]
    );
}

#[test]
fn a_deeper_claim_lists_its_next_segment() {
    let local = [plugin("api", &["serve", "api", "v2"], "API v2")];

    assert_eq!(names(&children(&["serve".to_owned()], &local, None)), [(
        "api", ""
    )]);
}

#[test]
fn a_group_renders_like_a_built_in_command_group() {
    let entries = [
        Entry {
            name: "http-api".to_owned(),
            description: "HTTP API for conversations (not installed)".to_owned(),
        },
        Entry {
            name: "web".to_owned(),
            description: "Web UI for conversations".to_owned(),
        },
    ];

    assert_eq!(
        render_group(&["serve".to_owned()], "JP server components", &entries),
        "\
JP server components

Usage: jp serve <COMMAND>

Commands:
  http-api        HTTP API for conversations (not installed)
  web             Web UI for conversations

Run `jp serve <command> -h` for more information.
"
    );
}

#[test]
fn an_empty_group_says_so() {
    assert_eq!(
        render_group(&["serve".to_owned()], "JP server components", &[]),
        "\
JP server components

Usage: jp serve <COMMAND>

No plugin provides a command here yet.

Run `jp serve <command> -h` for more information.
"
    );
}
