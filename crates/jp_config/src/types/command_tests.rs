use super::*;
use crate::assignment::KvAssignment;

#[test]
fn test_command_config_string_simple_split() {
    let p = PartialCommandConfigOrString::from_str("cargo check").unwrap();
    let cfg = CommandConfigOrString::from_partial(p, vec![]).unwrap();

    assert_eq!(cfg.command(), CommandConfig {
        program: "cargo".to_owned(),
        args: vec!["check".to_owned()],
        shell: false,
    });
}

#[test]
fn test_command_config_string_respects_single_quotes() {
    let p = PartialCommandConfigOrString::from_str("echo 'hello world'").unwrap();
    let cfg = CommandConfigOrString::from_partial(p, vec![]).unwrap();

    assert_eq!(cfg.command(), CommandConfig {
        program: "echo".to_owned(),
        args: vec!["hello world".to_owned()],
        shell: false,
    });
}

#[test]
fn test_command_config_string_respects_double_quotes() {
    let p = PartialCommandConfigOrString::from_str(r#"sh -c "ls -la""#).unwrap();
    let cfg = CommandConfigOrString::from_partial(p, vec![]).unwrap();

    assert_eq!(cfg.command(), CommandConfig {
        program: "sh".to_owned(),
        args: vec!["-c".to_owned(), "ls -la".to_owned()],
        shell: false,
    });
}

#[test]
fn test_command_config_string_handles_escapes() {
    let p = PartialCommandConfigOrString::from_str(r"echo hello\ world").unwrap();
    let cfg = CommandConfigOrString::from_partial(p, vec![]).unwrap();

    assert_eq!(cfg.command(), CommandConfig {
        program: "echo".to_owned(),
        args: vec!["hello world".to_owned()],
        shell: false,
    });
}

#[test]
fn test_command_config_string_rejects_unbalanced_quotes() {
    let err = PartialCommandConfigOrString::from_str("echo 'unterminated").unwrap_err();
    assert!(
        err.to_string().contains("invalid shell quoting"),
        "got: {err}"
    );
}

#[test]
fn test_command_config_string_empty_parses_to_empty_program() {
    let p = PartialCommandConfigOrString::from_str("").unwrap();
    let cfg = CommandConfigOrString::from_partial(p, vec![]).unwrap();

    // Empty input is accepted at config-parse time; the empty program
    // surfaces as a spawn-time error downstream, matching the legacy
    // `split_whitespace` behavior.
    assert_eq!(cfg.command(), CommandConfig {
        program: String::new(),
        args: vec![],
        shell: false,
    });
}

#[test]
fn test_command_config_structured_passthrough() {
    let mut p = PartialCommandConfigOrString::default();

    // `:` (with no preceding key) flags the value as raw JSON, leaving an
    // empty key for `PartialCommandConfigOrString::assign` to handle as a
    // structured object via `try_object_or_from_str`.
    let kv = KvAssignment::try_from_cli(
        ":",
        r#"{"program":"cargo","args":["check","--verbose"],"shell":true}"#,
    )
    .unwrap();
    p.assign(kv).unwrap();

    let cfg = CommandConfigOrString::from_partial(p, vec![]).unwrap();
    assert_eq!(cfg.command(), CommandConfig {
        program: "cargo".to_owned(),
        args: vec!["check".to_owned(), "--verbose".to_owned()],
        shell: true,
    });
}
