use pretty_assertions::assert_eq;

use super::*;

fn serve_web() -> Manifest {
    Manifest {
        protocol: 9,
        description: "Web UI".to_owned(),
        command: vec!["serve".to_owned(), "web".to_owned()],
    }
}

#[test]
fn a_manifest_is_found_between_binary_bytes() {
    let mut bytes = vec![0x7f, b'E', b'L', b'F', 0, 1, 2];
    bytes.extend_from_slice(
        br#"jp-plugin/v1 {"protocol":9,"description":"Web UI","command":["serve","web"]}"#,
    );
    bytes.extend_from_slice(&[0, 0xff, 0xfe]);

    assert_eq!(find(&bytes), Ok(Some(serve_web())));
}

#[test]
fn a_script_carries_its_manifest_in_a_comment() {
    let script = "#!/bin/sh\n# jp-plugin/v1 \
                  {\"protocol\":1,\"description\":\"Titles\",\"command\":[\"titles\"]}\necho hi\n";

    assert_eq!(
        find(script.as_bytes()),
        Ok(Some(Manifest {
            protocol: 1,
            description: "Titles".to_owned(),
            command: vec!["titles".to_owned()],
        }))
    );
}

#[test]
fn a_manifest_may_end_at_the_end_of_the_file() {
    let bytes = br#"jp-plugin/v1 {"protocol":9,"description":"Web UI","command":["serve","web"]}"#;

    assert_eq!(find(bytes), Ok(Some(serve_web())));
}

#[test]
fn a_file_without_a_manifest_has_none() {
    assert_eq!(find(b"#!/bin/sh\necho hi\n"), Ok(None));
}

/// The prefix alone, as it appears inside the host's own scanner, is not a
/// manifest.
#[test]
fn the_prefix_without_a_version_and_json_is_not_a_manifest() {
    assert_eq!(find(b"jp-plugin/v\0jp-plugin/v1\0jp-plugin/v1 x"), Ok(None));
}

#[test]
fn two_manifests_are_refused() {
    let bytes = br#"jp-plugin/v1 {"protocol":1,"description":"a","command":["a"]}
jp-plugin/v1 {"protocol":1,"description":"b","command":["b"]}"#;

    assert_eq!(find(bytes), Err(ManifestError::Duplicate));
}

#[test]
fn a_newer_framing_is_reported_as_such() {
    let bytes = br#"jp-plugin/v2 {"protocol":1,"description":"a","command":["a"]}"#;

    assert_eq!(find(bytes), Err(ManifestError::UnsupportedVersion(2)));
}

#[test]
fn keys_this_host_does_not_know_are_ignored() {
    let bytes = br#"jp-plugin/v1 {"protocol":9,"description":"Web UI","command":["serve","web"],"workspace_scope":"multi"}"#;

    assert_eq!(find(bytes), Ok(Some(serve_web())));
}

#[test]
fn an_escape_sequence_in_a_string_is_refused() {
    let bytes = br#"jp-plugin/v1 {"protocol":1,"description":"\u001b[2Jhi","command":["a"]}"#;

    assert_eq!(find(bytes), Err(ManifestError::ControlCharacter));
}

#[test]
fn a_control_character_in_an_unknown_key_is_refused() {
    let bytes = br#"jp-plugin/v1 {"protocol":1,"description":"a","command":["a"],"x":["\u0007"]}"#;

    assert_eq!(find(bytes), Err(ManifestError::ControlCharacter));
}

#[test]
fn newlines_and_tabs_in_a_string_are_allowed() {
    let bytes = br#"jp-plugin/v1 {"protocol":1,"description":"a\tb\nc","command":["a"]}"#;

    assert_eq!(find(bytes).unwrap().unwrap().description, "a\tb\nc");
}

#[test]
fn a_manifest_without_an_end_is_too_large() {
    let mut bytes = br#"jp-plugin/v1 {"protocol":1,"description":""#.to_vec();
    bytes.extend(std::iter::repeat_n(b'x', MAX_MANIFEST_SIZE));
    bytes.extend_from_slice(br#"","command":["a"]}"#);

    assert_eq!(find(&bytes), Err(ManifestError::TooLarge));
}

#[test]
fn invalid_utf8_is_refused() {
    let bytes = b"jp-plugin/v1 {\"protocol\":1,\"description\":\"\xff\",\"command\":[\"a\"]}";

    assert_eq!(find(bytes), Err(ManifestError::NotUtf8));
}

#[test]
fn a_missing_field_is_invalid() {
    let bytes = br#"jp-plugin/v1 {"protocol":1,"description":"a"}"#;

    assert!(matches!(find(bytes), Err(ManifestError::Invalid(_))));
}

#[test]
fn an_empty_command_is_invalid() {
    let bytes = br#"jp-plugin/v1 {"protocol":1,"description":"a","command":[]}"#;

    assert_eq!(
        find(bytes),
        Err(ManifestError::Invalid("`command` is empty".to_owned()))
    );
}

#[test]
fn a_segment_that_is_not_a_subcommand_name_is_invalid() {
    for segment in ["", "--flag", "two words"] {
        let json =
            format!(r#"jp-plugin/v1 {{"protocol":1,"description":"a","command":["{segment}"]}}"#);

        assert!(
            matches!(find(json.as_bytes()), Err(ManifestError::Invalid(_))),
            "{segment:?}"
        );
    }
}

#[test]
fn a_segment_may_contain_dashes() {
    let bytes = br#"jp-plugin/v1 {"protocol":1,"description":"a","command":["serve","http-api"]}"#;

    assert_eq!(find(bytes).unwrap().unwrap().command, ["serve", "http-api"]);
}

#[test]
fn fields_from_elsewhere_are_checked_like_a_file() {
    assert_eq!(serve_web().check(), Ok(()));

    let escape = Manifest {
        description: "\u{1b}[2J".to_owned(),
        ..serve_web()
    };
    assert_eq!(escape.check(), Err(ManifestError::ControlCharacter));

    let empty = Manifest {
        command: vec![],
        ..serve_web()
    };
    assert_eq!(
        empty.check(),
        Err(ManifestError::Invalid("`command` is empty".to_owned()))
    );
}

#[test]
fn the_macro_builds_a_line_the_host_finds() {
    const LINE: &str = crate::manifest!(
        protocol: 9,
        description: "A \"quoted\" description",
        command: ["serve", "web"],
    );

    assert_eq!(
        LINE,
        "jp-plugin/v1 {\"protocol\":9,\"description\":\"A \\\"quoted\\\" \
         description\",\"command\":[\"serve\", \"web\"]}\n"
    );

    let expected = Manifest {
        protocol: 9,
        description: "A \"quoted\" description".to_owned(),
        command: vec!["serve".to_owned(), "web".to_owned()],
    };
    assert_eq!(find(LINE.as_bytes()), Ok(Some(expected.clone())));
    assert_eq!(Manifest::from_line(LINE), Ok(expected));
}
