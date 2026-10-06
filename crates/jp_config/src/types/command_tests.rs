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

fn command(program: &str, args: &[&str], shell: bool) -> CommandConfig {
    CommandConfig {
        program: program.to_owned(),
        args: args.iter().map(|s| (*s).to_owned()).collect(),
        shell,
    }
}

#[test]
fn shell_command_line_no_args_is_program_verbatim() {
    // The program is shell syntax and must pass through untouched.
    assert_eq!(
        command("foo | bar", &[], true).shell_command_line(),
        "foo | bar"
    );
}

#[test]
fn shell_command_line_quotes_multiword_args() {
    let cmd = command("grep", &["foo bar", "file"], false);

    assert_eq!(cmd.shell_command_line(), "grep 'foo bar' file");
}

#[test]
fn shell_command_line_keeps_program_raw() {
    // Only the discrete args are quoted; the program stays verbatim.
    assert_eq!(
        command("a && b", &["c"], true).shell_command_line(),
        "a && b c"
    );
}

/// `sh -c` receives the command line on its own, so a wrapper added here would
/// nest one shell inside another.
#[test]
fn shell_command_line_carries_no_shell_wrapper() {
    let line = "git rev-parse --show-toplevel";

    assert_eq!(command(line, &[], true).shell_command_line(), line);
}

/// A directly spawned command reads as the line the user configured.
#[test]
fn display_shows_the_command_line() {
    assert_eq!(
        command("code", &["--wait"], false).to_string(),
        "code --wait"
    );
}

/// A program path is one literal token however many spaces it contains, so it
/// renders quoted: pasting the line into a shell runs the same program.
#[test]
fn display_quotes_a_direct_program_path_with_spaces() {
    let cmd = command(
        "/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code",
        &["--wait"],
        false,
    );

    assert_eq!(
        cmd.to_string(),
        "'/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code' --wait"
    );
}

/// A shell command names the shell that interprets it, so a reader can tell
/// that `|`, `&&`, and `$(...)` in the line are live rather than literal.
#[test]
fn display_names_the_shell_for_a_shell_command() {
    let cmd = command(r#"basename "$(git rev-parse --show-toplevel)""#, &[], true);

    assert_eq!(
        cmd.to_string(),
        r#"sh -c 'basename "$(git rev-parse --show-toplevel)"'"#
    );
}

/// The same text means different things with and without a shell, so the two
/// must not render alike.
#[test]
fn display_distinguishes_a_shell_command_from_a_direct_one() {
    let line = "foo | bar";

    assert_ne!(
        command(line, &[], true).to_string(),
        command(line, &[], false).to_string()
    );
}

/// A script containing single quotes stays one `sh -c` operand: the quoting
/// switches to double quotes rather than terminating early and spilling the
/// rest of the script into further operands.
#[test]
fn display_keeps_a_quoted_script_in_one_operand() {
    let cmd = command("echo 'hi'", &[], true);

    assert_eq!(cmd.to_string(), r#"sh -c "echo 'hi'""#);
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
