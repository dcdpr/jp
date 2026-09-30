use jp_plugin::Manifest;
use pretty_assertions::assert_eq;

use super::*;

#[test]
fn describe_answers_from_the_embedded_manifest() {
    let mut out = Vec::new();
    send_describe(&mut out).unwrap();

    let PluginToHost::Describe(describe) = serde_json::from_slice(&out).unwrap() else {
        panic!("expected a describe answer");
    };

    assert_eq!(describe.manifest, Manifest {
        protocol: 9,
        description: "Web UI for browsing conversations and continuing them".to_owned(),
        command: vec!["serve".to_owned(), "web".to_owned()],
    });
    assert_eq!(describe.name, "serve-web");
    assert_eq!(describe.help, HELP_TEXT);
}

/// The host refuses a plugin before spawning it when the manifest asks for a
/// newer protocol, so the manifest has to state the same need the handshake
/// does.
#[test]
fn the_manifest_states_the_protocol_the_handshake_needs() {
    let manifest = jp_plugin::manifest::find(MANIFEST.as_bytes())
        .unwrap()
        .unwrap();

    assert_eq!(manifest.protocol, REQUIRED_PROTOCOL);
}
