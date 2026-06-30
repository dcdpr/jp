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
fn shell_command_line_no_args_is_program_verbatim() {
    // The program is shell syntax and must pass through untouched.
    assert_eq!(shell_command_line("foo | bar", &[]), "foo | bar");
}

#[test]
fn shell_command_line_quotes_multiword_args() {
    let line = shell_command_line("grep", &["foo bar".to_owned(), "file".to_owned()]);
    assert_eq!(line, "grep 'foo bar' file");
}

#[test]
fn shell_command_line_keeps_program_raw() {
    // Only the discrete args are quoted; the program stays verbatim.
    let line = shell_command_line("a && b", &["c".to_owned()]);
    assert_eq!(line, "a && b c");
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
