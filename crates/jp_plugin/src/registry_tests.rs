use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

#[test]
fn registry_roundtrip() {
    let registry = Registry {
        version: 1,
        plugins: [("serve".to_owned(), RegistryPlugin {
            id: "serve".to_owned(),
            kind: PluginKind::Command {
                requires: vec![],
                suggests: vec![],
                binaries: [("aarch64-apple-darwin".to_owned(), RegistryBinary {
                    url: "https://example.com/jp-serve-aarch64-apple-darwin".to_owned(),
                    sha256: "abcdef1234567890".to_owned(),
                })]
                .into_iter()
                .collect(),
            },
            description: "Read-only web UI for conversations".to_owned(),
            official: true,
            repository: Some("https://github.com/dcdpr/jp".to_owned()),
        })]
        .into_iter()
        .collect(),
    };

    let json = serde_json::to_string(&registry).unwrap();
    let parsed: Registry = serde_json::from_str(&json).unwrap();
    assert_eq!(registry, parsed);
}

#[test]
fn registry_deserializes_with_space_separated_keys() {
    let json = json!({
        "version": 1,
        "plugins": {
            "serve web": {
                "id": "serve-web",
                "type": "command",
                "description": "Read-only web UI for conversations",
                "official": true,
                "repository": "https://github.com/dcdpr/jp",
                "binaries": {
                    "aarch64-apple-darwin": {
                        "url": "https://example.com/aarch64",
                        "sha256": "def456"
                    }
                }
            }
        }
    });

    let registry: Registry = serde_json::from_value(json).unwrap();
    assert_eq!(registry.version, 1);
    assert_eq!(registry.plugins.len(), 1);

    let web = &registry.plugins["serve web"];
    assert_eq!(web.id, "serve-web");
    assert!(web.official);
    assert_eq!(web.kind.binaries().len(), 1);
}

#[test]
fn defaults_for_optional_fields() {
    let json = json!({
        "id": "test",
        "type": "command",
        "description": "A test plugin"
    });

    let plugin: RegistryPlugin = serde_json::from_value(json).unwrap();
    assert_eq!(plugin.id, "test");
    assert!(plugin.kind.is_command());
    assert!(!plugin.official);
    assert!(plugin.repository.is_none());
    assert!(plugin.kind.binaries().is_empty());

    let PluginKind::Command {
        requires, suggests, ..
    } = &plugin.kind
    else {
        panic!("expected Command");
    };
    assert!(requires.is_empty());
    assert!(suggests.is_empty());
}

#[test]
fn optional_fields_omitted_from_json() {
    let plugin = RegistryPlugin {
        id: "test".to_owned(),
        kind: PluginKind::default(),
        description: "Test".to_owned(),
        official: false,
        repository: None,
    };

    let json = serde_json::to_value(&plugin).unwrap();
    assert!(json.get("repository").is_none());
    assert!(json.get("requires").is_none());
    assert!(json.get("suggests").is_none());
    assert!(json.get("binaries").is_none());
}

#[test]
fn command_group_with_suggests() {
    let json = json!({
        "version": 1,
        "plugins": {
            "serve": {
                "id": "serve",
                "type": "command_group",
                "description": "JP server components",
                "official": true,
                "suggests": ["serve web", "serve http-api"]
            },
            "serve web": {
                "id": "serve-web",
                "type": "command",
                "description": "Web UI for conversations",
                "official": true,
                "requires": ["serve"],
                "binaries": {
                    "aarch64-apple-darwin": {
                        "url": "https://example.com/serve-web",
                        "sha256": "abc123"
                    }
                }
            }
        }
    });

    let registry: Registry = serde_json::from_value(json).unwrap();

    let serve = &registry.plugins["serve"];
    assert_eq!(serve.id, "serve");
    assert!(serve.kind.is_command_group());

    let PluginKind::CommandGroup { suggests } = &serve.kind else {
        panic!("expected CommandGroup");
    };
    assert_eq!(suggests, &["serve web", "serve http-api"]);

    let web = &registry.plugins["serve web"];
    assert_eq!(web.id, "serve-web");
    assert!(web.kind.is_command());

    let PluginKind::Command {
        requires, suggests, ..
    } = &web.kind
    else {
        panic!("expected Command");
    };
    assert_eq!(requires, &["serve"]);
    assert!(suggests.is_empty());
    assert_eq!(web.kind.binaries().len(), 1);
}

#[test]
fn approvals_roundtrip() {
    let approvals = PluginApprovals {
        approved: [("dashboard".to_owned(), ApprovedPlugin {
            path: "/usr/local/bin/jp-dashboard".into(),
            sha256: "abc123def456".to_owned(),
        })]
        .into_iter()
        .collect(),
    };

    let json = serde_json::to_string(&approvals).unwrap();
    let parsed: PluginApprovals = serde_json::from_str(&json).unwrap();
    assert_eq!(approvals, parsed);
}

#[test]
fn approvals_default_is_empty() {
    let approvals = PluginApprovals::default();
    assert!(approvals.approved.is_empty());
}

#[test]
fn empty_json_deserializes_to_default_approvals() {
    let parsed: PluginApprovals = serde_json::from_str("{}").unwrap();
    assert!(parsed.approved.is_empty());
}
