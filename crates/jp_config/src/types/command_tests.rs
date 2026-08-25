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

/// Expanding a shorthand yields the same command it would have run.
///
/// This is the invariant that makes expanding-on-sub-key-assignment safe: if
/// the two ever diverged, addressing a field would silently change the command.
///
/// The template spans are the cases that matter most: they are the only inputs
/// where the shell split alone gives a different answer than
/// [`CommandConfigOrString::command`], so an expansion that skipped the
/// template-aware splitter would pass every other case here.
#[test]
fn expanding_a_shorthand_matches_the_command_it_describes() {
    for shorthand in [
        "cargo check",
        "echo 'hello world'",
        r#"sh -c "ls -la""#,
        "code",
        "",
        "just x {{ a | default('') }}",
        "echo {% if x %}on{% endif %} tail",
        "echo {# a note #}",
    ] {
        let expanded = CommandConfigOrString::from_partial(
            PartialCommandConfigOrString::Config(expand_shorthand(shorthand)),
            vec![],
        )
        .expect("the expansion is a valid config");

        let direct = CommandConfigOrString::String(shorthand.to_owned());

        assert_eq!(
            expanded.command(),
            direct.command(),
            "expanding {shorthand:?} changed the command"
        );
    }
}

/// A field of the table form is addressable even when a shorthand was written,
/// and the program the shorthand named survives.
#[test]
fn assigning_a_field_expands_the_shorthand() {
    let mut p = PartialCommandConfigOrString::String("code --wait".to_owned());

    let kv = KvAssignment::try_from_cli("shell", "true").unwrap();
    p.assign(kv).unwrap();

    let cfg = CommandConfigOrString::from_partial(p, vec![]).unwrap();
    assert_eq!(cfg.command(), CommandConfig {
        program: "code".to_owned(),
        args: vec!["--wait".to_owned()],
        shell: true,
    });
}

/// Assigning `args` replaces the shorthand's arguments while keeping its
/// program.
#[test]
fn assigning_args_keeps_the_shorthand_program() {
    let mut p = PartialCommandConfigOrString::String("code --wait".to_owned());

    let kv = KvAssignment::try_from_cli("args:", r#"["--foo"]"#).unwrap();
    p.assign(kv).unwrap();

    let cfg = CommandConfigOrString::from_partial(p, vec![]).unwrap();
    assert_eq!(cfg.command(), CommandConfig {
        program: "code".to_owned(),
        args: vec!["--foo".to_owned()],
        shell: false,
    });
}

/// Assigning a field to a fresh partial works, which is the shape environment
/// variables arrive in: they are assigned onto an empty partial and merged.
#[test]
fn assigning_a_field_to_a_default_partial() {
    let mut p = PartialCommandConfigOrString::default();

    let kv = KvAssignment::try_from_cli("program", "code").unwrap();
    p.assign(kv).unwrap();

    let cfg = CommandConfigOrString::from_partial(p, vec![]).unwrap();
    assert_eq!(cfg.command(), CommandConfig {
        program: "code".to_owned(),
        args: vec![],
        shell: false,
    });
}

/// Writing the whole value after a field replaces it, so the order of `--cfg`
/// arguments matters once fields are addressed.
#[test]
fn a_whole_value_assignment_replaces_earlier_fields() {
    let mut p = PartialCommandConfigOrString::default();

    let kv = KvAssignment::try_from_cli("args:", r#"["--wait"]"#).unwrap();
    p.assign(kv).unwrap();

    let kv = KvAssignment::try_from_cli("", "code").unwrap();
    p.assign(kv).unwrap();

    let cfg = CommandConfigOrString::from_partial(p, vec![]).unwrap();
    assert_eq!(
        cfg.command(),
        CommandConfig {
            program: "code".to_owned(),
            args: vec![],
            shell: false,
        },
        "the later whole-value write wins outright"
    );
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

#[test]
fn template_span_with_spaces_stays_one_arg() {
    // A template expression with interior spaces (and a quoted filter arg) must
    // survive the shell split as a single argument.
    let p =
        PartialCommandConfigOrString::from_str("just rfd-renumber {{ a }} {{ b | default('') }}")
            .unwrap();
    let cfg = CommandConfigOrString::from_partial(p, vec![]).unwrap();

    assert_eq!(cfg.command(), CommandConfig {
        program: "just".to_owned(),
        args: vec![
            "rfd-renumber".to_owned(),
            "{{ a }}".to_owned(),
            "{{ b | default('') }}".to_owned(),
        ],
        shell: false,
    });
}

#[test]
fn template_statement_and_comment_spans_are_atomic() {
    let p = PartialCommandConfigOrString::from_str("run {% if x %} {# note #}").unwrap();
    let cfg = CommandConfigOrString::from_partial(p, vec![]).unwrap();

    assert_eq!(cfg.command(), CommandConfig {
        program: "run".to_owned(),
        args: vec!["{% if x %}".to_owned(), "{# note #}".to_owned()],
        shell: false,
    });
}

#[test]
fn template_span_adjacent_to_literal_text() {
    let p = PartialCommandConfigOrString::from_str("cmd pre{{ x }}post").unwrap();
    let cfg = CommandConfigOrString::from_partial(p, vec![]).unwrap();

    assert_eq!(cfg.command(), CommandConfig {
        program: "cmd".to_owned(),
        args: vec!["pre{{ x }}post".to_owned()],
        shell: false,
    });
}

#[test]
fn template_span_closer_inside_string_literal() {
    // The `}}` inside the quoted string must not end the span early.
    let p = PartialCommandConfigOrString::from_str(r#"echo {{ "}}" }}"#).unwrap();
    let cfg = CommandConfigOrString::from_partial(p, vec![]).unwrap();

    assert_eq!(cfg.command(), CommandConfig {
        program: "echo".to_owned(),
        args: vec![r#"{{ "}}" }}"#.to_owned()],
        shell: false,
    });
}

#[test]
fn unterminated_template_span_is_kept_whole() {
    // An unterminated `{{` swallows the rest; minijinja reports the real error
    // at render time rather than the splitter mangling it.
    let p = PartialCommandConfigOrString::from_str("just x {{ a").unwrap();
    let cfg = CommandConfigOrString::from_partial(p, vec![]).unwrap();

    assert_eq!(cfg.command(), CommandConfig {
        program: "just".to_owned(),
        args: vec!["x".to_owned(), "{{ a".to_owned()],
        shell: false,
    });
}

#[test]
fn template_span_with_nested_braces_stays_one_arg() {
    // Minijinja only ends a variable block at nesting depth zero, so the `}}`
    // that closes the two maps must not end the span.
    let p = PartialCommandConfigOrString::from_str(r#"cmd {{ {"outer": {"inner": 1}} | tojson }}"#)
        .unwrap();
    let cfg = CommandConfigOrString::from_partial(p, vec![]).unwrap();

    assert_eq!(cfg.command(), CommandConfig {
        program: "cmd".to_owned(),
        args: vec![r#"{{ {"outer": {"inner": 1}} | tojson }}"#.to_owned()],
        shell: false,
    });
}

#[test]
fn template_span_ends_at_first_depth_zero_closer() {
    // The span ends at the `}}` that follows the balanced map, and text after it
    // is split as ordinary shell words.
    let p = PartialCommandConfigOrString::from_str(r#"cmd {{ {"a": 1} }} tail"#).unwrap();
    let cfg = CommandConfigOrString::from_partial(p, vec![]).unwrap();

    assert_eq!(cfg.command(), CommandConfig {
        program: "cmd".to_owned(),
        args: vec![r#"{{ {"a": 1} }}"#.to_owned(), "tail".to_owned()],
        shell: false,
    });
}

#[test]
fn from_str_rejects_nul_byte() {
    let err = PartialCommandConfigOrString::from_str("echo \0").unwrap_err();
    assert!(err.to_string().contains("NUL byte"), "got: {err}");
}

#[test]
fn json_nul_bearing_command_is_not_rewritten_by_span_restoration() {
    // JSON (and YAML, and JSON5) can encode an interior NUL, and that path
    // deserializes the string variant directly without going through `from_str`.
    // Text shaped like a span placeholder must not be substituted with real span
    // text: the command stays unexecutable instead of turning into a different
    // one.
    let p: PartialCommandConfigOrString =
        serde_json::from_str(r#""\u00000\u0000 {{ evil }}""#).unwrap();
    let cfg = CommandConfigOrString::from_partial(p, vec![]).unwrap();

    assert_eq!(cfg.command(), CommandConfig {
        program: String::new(),
        args: vec![],
        shell: false,
    });
}

#[test]
fn from_str_ignores_quotes_inside_template_span() {
    // The apostrophe inside the comment is minijinja text, not shell quoting;
    // masking the span keeps the unbalanced-quote check from rejecting it.
    let p = PartialCommandConfigOrString::from_str("echo {# don't split #}").unwrap();
    let cfg = CommandConfigOrString::from_partial(p, vec![]).unwrap();

    assert_eq!(cfg.command(), CommandConfig {
        program: "echo".to_owned(),
        args: vec!["{# don't split #}".to_owned()],
        shell: false,
    });
}
