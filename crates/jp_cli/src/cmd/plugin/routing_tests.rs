use std::collections::BTreeMap;

use jp_plugin::{
    Manifest,
    registry::{PluginKind, Registry, RegistryPlugin},
};
use pretty_assertions::assert_eq;

use super::*;
use crate::cmd::plugin::discovery::{Location, ManifestState};

fn args(line: &str) -> Vec<String> {
    line.split(' ').map(ToOwned::to_owned).collect()
}

fn plugin(name: &str, dir: &str, command: &[&str]) -> LocalPlugin {
    LocalPlugin {
        name: name.to_owned(),
        path: Utf8PathBuf::from(format!("{dir}/jp-{name}")),
        location: if dir == "/install" {
            Location::InstallDir
        } else {
            Location::Path
        },
        manifest: ManifestState::Valid(Manifest {
            protocol: 1,
            description: format!("{name} plugin"),
            command: command.iter().map(|s| (*s).to_owned()).collect(),
        }),
    }
}

fn without_manifest(name: &str, dir: &str) -> LocalPlugin {
    LocalPlugin {
        manifest: ManifestState::Missing,
        ..plugin(name, dir, &["unused"])
    }
}

fn command(id: &str, official: bool) -> RegistryPlugin {
    RegistryPlugin {
        id: id.to_owned(),
        description: format!("{id} from the registry"),
        official,
        repository: None,
        kind: PluginKind::default(),
    }
}

fn group(id: &str) -> RegistryPlugin {
    RegistryPlugin {
        id: id.to_owned(),
        description: "JP server components".to_owned(),
        official: true,
        repository: None,
        kind: PluginKind::CommandGroup {
            suggests: vec!["serve web".to_owned()],
        },
    }
}

/// The published registry: the `serve` group, the official `serve web`, and a
/// third-party `metrics`.
fn registry() -> Registry {
    Registry {
        version: 1,
        plugins: BTreeMap::from([
            ("serve".to_owned(), group("serve")),
            ("serve web".to_owned(), command("serve-web", true)),
            ("metrics".to_owned(), command("metrics", false)),
        ]),
    }
}

#[test]
fn a_plugin_is_routed_by_its_manifest_not_its_file_name() {
    let local = [plugin("webui", "/bin", &["serve", "web"])];

    let route = route(&args("serve web --port 1"), &local, None).unwrap();

    assert_eq!(route, Route::Local {
        plugin: &local[0],
        consumed: 2,
        replaces: None,
    });
}

#[test]
fn the_longest_claimed_path_wins() {
    let local = [
        plugin("serve", "/bin", &["serve"]),
        plugin("webui", "/bin", &["serve", "web"]),
    ];

    let Route::Local {
        plugin, consumed, ..
    } = route(&args("serve web"), &local, None).unwrap()
    else {
        panic!("expected a local route");
    };
    assert_eq!((plugin.name.as_str(), consumed), ("webui", 2));

    // Anything else under `serve` is the shorter claim's to handle.
    let Route::Local {
        plugin, consumed, ..
    } = route(&args("serve api"), &local, None).unwrap()
    else {
        panic!("expected a local route");
    };
    assert_eq!((plugin.name.as_str(), consumed), ("serve", 1));
}

#[test]
fn a_third_party_binary_replaces_the_official_command() {
    let registry = registry();
    let local = [plugin("webui", "/bin", &["serve", "web"])];

    let route = route(&args("serve web"), &local, Some(&registry)).unwrap();

    assert_eq!(route, Route::Local {
        plugin: &local[0],
        consumed: 2,
        replaces: Some("serve web"),
    });
}

#[test]
fn two_third_party_claimants_are_refused() {
    let local = [
        plugin("webui", "/bin", &["serve", "web"]),
        plugin("other", "/usr/bin", &["serve", "web"]),
    ];

    assert_eq!(
        route(&args("serve web"), &local, None),
        Err(RouteError::Ambiguous {
            command: "serve web".to_owned(),
            paths: vec!["/bin/jp-webui".into(), "/usr/bin/jp-other".into()],
        })
    );
}

#[test]
fn two_binaries_with_one_name_are_refused() {
    let local = [
        plugin("titles", "/bin", &["titles"]),
        plugin("titles", "/usr/bin", &["other"]),
    ];

    assert_eq!(
        route(&args("titles"), &local, None),
        Err(RouteError::SameName {
            name: "titles".to_owned(),
            paths: vec!["/bin/jp-titles".into(), "/usr/bin/jp-titles".into()],
        })
    );
}

#[test]
fn an_official_plugin_that_is_not_installed_is_routed_to_the_registry() {
    let registry = registry();

    let route = route(&args("serve web"), &[], Some(&registry)).unwrap();

    assert_eq!(route, Route::Official {
        key: "serve web",
        entry: &registry.plugins["serve web"],
        binary: None,
        consumed: 2,
    });
}

#[test]
fn an_installed_official_plugin_runs_its_binary() {
    let registry = registry();
    let local = [plugin("serve-web", "/install", &["serve", "web"])];

    let Route::Official { binary, .. } =
        route(&args("serve web"), &local, Some(&registry)).unwrap()
    else {
        panic!("expected an official route");
    };

    assert_eq!(binary, Some(&local[0]));
}

#[test]
fn an_official_binary_without_a_manifest_is_refused() {
    let registry = registry();
    let local = [without_manifest("serve-web", "/install")];

    assert_eq!(
        route(&args("serve web"), &local, Some(&registry)),
        Err(RouteError::InvalidManifest {
            name: "serve-web".to_owned(),
            path: "/install/jp-serve-web".into(),
            reason: "no plugin manifest".to_owned(),
            command: "serve web".to_owned(),
        })
    );
}

#[test]
fn an_official_binary_with_an_unusable_manifest_is_refused() {
    let registry = registry();
    let local = [LocalPlugin {
        manifest: ManifestState::Invalid("too new".to_owned()),
        ..without_manifest("serve-web", "/install")
    }];

    assert_eq!(
        route(&args("serve web"), &local, Some(&registry)),
        Err(RouteError::InvalidManifest {
            name: "serve-web".to_owned(),
            path: "/install/jp-serve-web".into(),
            reason: "invalid manifest: too new".to_owned(),
            command: "serve web".to_owned(),
        })
    );
}

#[test]
fn an_official_binary_claiming_another_path_is_refused_under_either_path() {
    let registry = registry();
    let local = [plugin("serve-web", "/install", &["web"])];

    let expected = RouteError::OfficialMismatch {
        name: "serve-web".to_owned(),
        path: "/install/jp-serve-web".into(),
        claimed: "web".to_owned(),
        expected: "serve web".to_owned(),
    };

    assert_eq!(
        route(&args("serve web"), &local, Some(&registry)),
        Err(expected.clone())
    );
    assert_eq!(route(&args("web"), &local, Some(&registry)), Err(expected));
}

#[test]
fn a_command_group_is_routed_when_nothing_longer_matches() {
    let registry = registry();

    assert_eq!(
        route(&args("serve"), &[], Some(&registry)).unwrap(),
        Route::Group {
            key: "serve",
            entry: &registry.plugins["serve"],
            consumed: 1,
        }
    );
    assert!(matches!(
        route(&args("serve web"), &[], Some(&registry)).unwrap(),
        Route::Official { .. }
    ));
}

#[test]
fn a_third_party_registry_entry_claims_nothing_but_is_named() {
    let registry = registry();

    assert_eq!(
        route(&args("metrics --port 9"), &[], Some(&registry)).unwrap(),
        Route::ThirdParty {
            key: "metrics",
            entry: &registry.plugins["metrics"],
        }
    );
}

#[test]
fn an_installed_third_party_plugin_is_routed_like_any_other() {
    let registry = registry();
    let local = [plugin("metrics", "/install", &["metrics"])];

    assert!(matches!(
        route(&args("metrics"), &local, Some(&registry)).unwrap(),
        Route::Local { replaces: None, .. }
    ));
}

#[test]
fn a_binary_without_a_manifest_claims_nothing() {
    let local = [without_manifest("titles", "/bin")];

    assert_eq!(
        route(&args("titles"), &local, None).unwrap(),
        Route::NotFound
    );
}

#[test]
fn a_partial_path_is_not_a_claim() {
    let local = [plugin("webui", "/bin", &["serve", "web"])];

    assert_eq!(
        route(&args("serve"), &local, None).unwrap(),
        Route::NotFound
    );
    assert_eq!(
        route(&args("serve webs"), &local, None).unwrap(),
        Route::NotFound
    );
}
