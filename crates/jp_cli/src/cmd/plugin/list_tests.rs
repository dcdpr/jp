use std::collections::BTreeMap;

use camino::Utf8PathBuf;
use jp_plugin::{
    Manifest,
    registry::{PluginKind, RegistryPlugin},
};
use pretty_assertions::assert_eq;

use super::*;
use crate::cmd::plugin::discovery::{Location, ManifestState};

fn plugin(name: &str, manifest: ManifestState) -> LocalPlugin {
    LocalPlugin {
        name: name.to_owned(),
        path: Utf8PathBuf::from(format!("/bin/jp-{name}")),
        location: Location::Path,
        manifest,
    }
}

fn claims(command: &[&str]) -> ManifestState {
    ManifestState::Valid(Manifest {
        protocol: 1,
        description: String::new(),
        command: command.iter().map(|s| (*s).to_owned()).collect(),
    })
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

#[test]
fn installed_plugins_and_what_the_registry_offers() {
    let registry = Registry {
        version: 1,
        plugins: BTreeMap::from([
            ("path".to_owned(), entry("path", true, "Print paths")),
            ("serve".to_owned(), RegistryPlugin {
                kind: PluginKind::CommandGroup { suggests: vec![] },
                ..entry("serve", true, "Servers")
            }),
            ("serve web".to_owned(), entry("serve-web", true, "Web UI")),
            ("metrics".to_owned(), entry("metrics", false, "Exporter")),
        ]),
    };
    let path = plugin("path", claims(&["path"]));
    let webui = plugin("webui", claims(&["serve", "web"]));
    let tools = plugin("tools", ManifestState::Missing);
    let rows = [
        (&path, "official release".to_owned()),
        (&webui, "approved".to_owned()),
        (&tools, "not approved".to_owned()),
    ];

    assert_eq!(
        render(&rows, Some(&registry)),
        "\
Installed:
  path             jp path              /bin/jp-path (official, official release)
  webui            jp serve web         /bin/jp-webui (third-party, approved, replaces the \
         official `jp serve web`)
  tools            -                    /bin/jp-tools (third-party, not approved, no plugin \
         manifest)

Available:
  metrics          jp metrics           Exporter (command, third-party, `jp plugin install \
         metrics`)
  serve            jp serve             Servers (command group, official)
  serve-web        jp serve web         Web UI (command, official, installs on first use)
"
    );
}

#[test]
fn two_binaries_with_one_name_are_marked() {
    let first = plugin("titles", claims(&["titles"]));
    let second = LocalPlugin {
        path: "/usr/bin/jp-titles".into(),
        ..plugin("titles", claims(&["titles"]))
    };
    let rows = [
        (&first, "approved".to_owned()),
        (&second, "another binary is approved".to_owned()),
    ];

    assert_eq!(
        render(&rows, None),
        "\
Installed:
  titles           jp titles            /bin/jp-titles (third-party, approved, another binary has \
         the same name)
  titles           jp titles            /usr/bin/jp-titles (third-party, another binary is \
         approved, another binary has the same name)
"
    );
}

#[test]
fn nothing_to_list_says_how_to_get_the_registry() {
    assert_eq!(
        render(&[], None),
        "No plugins found. Run `jp plugin update` to fetch the registry.\n"
    );
}

#[test]
fn the_approval_state_says_why_a_binary_runs_or_asks() {
    let state = |release, sha256, approval, installed| {
        approval_state(release, sha256, &approval, installed)
    };

    assert_eq!(
        state(Some("a"), "a", ApprovalMatch::None, false),
        "official release"
    );
    assert_eq!(
        state(Some("b"), "a", ApprovalMatch::Matches, true),
        "installed by jp"
    );
    assert_eq!(state(None, "a", ApprovalMatch::Matches, false), "approved");
    assert_eq!(
        state(None, "a", ApprovalMatch::Changed, false),
        "changed since approved"
    );
    assert_eq!(
        state(None, "a", ApprovalMatch::Elsewhere("/x".into()), false),
        "another binary is approved"
    );
    assert_eq!(
        state(Some("b"), "a", ApprovalMatch::None, false),
        "not the official release"
    );
    assert_eq!(state(None, "a", ApprovalMatch::None, false), "not approved");
}
