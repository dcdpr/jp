use jp_plugin::Manifest;

use super::*;

#[test]
fn describe_answers_from_the_embedded_manifest() {
    let mut out = Vec::new();
    send_describe(&mut out).unwrap();

    let msg: PluginToHost = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        msg,
        PluginToHost::Describe(DescribeResponse {
            manifest: Manifest {
                protocol: 1,
                description: "Print JP directory paths".to_owned(),
                command: vec!["path".to_owned()],
            },
            name: "path".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            help: HELP_TEXT.to_owned(),
            author: Some("Jean Mertz <git@jeanmertz.com>".to_owned()),
            repository: Some("https://github.com/dcdpr/jp".to_owned()),
        })
    );
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
