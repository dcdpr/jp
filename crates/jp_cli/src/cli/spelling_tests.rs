use clap::{Arg, ArgAction};

use super::*;

/// A command with an optional-value option `--opt` / `-o`, a flag `-x`, an
/// option `--raw` whose value may look like a flag, a root option `--cfg` that
/// requires a value, and a subcommand `sub` taking words.
fn command() -> Command {
    let opt = Arg::new("opt")
        .short('o')
        .long("opt")
        .alias("option")
        .action(ArgAction::Append)
        .num_args(0..=1)
        .default_missing_value("");

    let mut cmd = Command::new("prog")
        .allow_external_subcommands(true)
        .arg(Arg::new("cfg").long("cfg").action(ArgAction::Append))
        .subcommand(
            Command::new("sub")
                .alias("s")
                .arg(opt)
                .arg(Arg::new("raw").long("raw").allow_hyphen_values(true))
                .arg(Arg::new("x").short('x').action(ArgAction::SetTrue))
                .arg(Arg::new("words").action(ArgAction::Append).num_args(1..)),
        );
    cmd.build();
    cmd
}

/// How `argv` (after the program name) spelled each occurrence of `id`.
fn attached(argv: &[&str], id: &str) -> Vec<bool> {
    let argv: Vec<OsString> = ["prog"].iter().chain(argv).map(OsString::from).collect();

    let mut cmd = command();
    let matches = cmd.try_get_matches_from_mut(&argv).unwrap();

    // The occurrences clap counted, at the level of the command that declares
    // `id`.
    let mut level = Some(&matches);
    let mut occurrences = 0;
    while let Some(matches) = level {
        if let Ok(Some(found)) = matches.try_get_raw_occurrences(id) {
            occurrences = found.count();
            break;
        }
        level = matches.subcommand().map(|(_, sub)| sub);
    }

    Spelling::of(&cmd, &matches, &argv).attached(id, occurrences)
}

#[test]
fn a_long_value_is_attached_only_after_an_equals_sign() {
    assert_eq!(attached(&["sub", "--opt=a", "--opt", "b"], "opt"), [
        true, false
    ]);
}

#[test]
fn a_short_value_is_attached_when_it_shares_the_argument() {
    assert_eq!(attached(&["sub", "-oa", "-o=a", "-o", "a"], "opt"), [
        true, true, false
    ]);
}

#[test]
fn a_short_cluster_ends_at_the_option_that_takes_a_value() {
    assert_eq!(attached(&["sub", "-xo", "a"], "opt"), [false]);
    assert_eq!(attached(&["sub", "-xoa"], "opt"), [true]);
}

#[test]
fn an_option_written_without_a_value_is_not_attached() {
    assert_eq!(attached(&["sub", "words", "-o"], "opt"), [false]);
}

#[test]
fn an_alias_is_recognized() {
    assert_eq!(attached(&["sub", "--option=a", "--option", "b"], "opt"), [
        true, false
    ]);
}

// clap reads everything after `--` as words, however it looks.
#[test]
fn nothing_after_a_double_dash_is_an_option() {
    assert_eq!(attached(&["sub", "-o", "a", "--", "-o=b"], "opt"), [false]);
}

// `--cfg` takes the next argument even when it names a subcommand, so the walk
// must not descend into `sub` there.
#[test]
fn a_required_value_is_never_mistaken_for_a_subcommand() {
    assert_eq!(attached(&["--cfg", "sub", "sub", "-o=x"], "opt"), [true]);
    assert_eq!(attached(&["--cfg", "sub", "sub", "-o=x"], "cfg"), [false]);
}

#[test]
fn a_subcommand_alias_is_descended_into() {
    assert_eq!(attached(&["s", "-o", "a", "words"], "opt"), [false]);
}

// An optional value does not take a following option as its value, so the
// option after it is read as an option of its own.
#[test]
fn an_optional_value_does_not_consume_a_following_option() {
    assert_eq!(attached(&["sub", "-o", "-o=b"], "opt"), [false, true]);
}

// Everything after a plugin's command name is the plugin's to read, so none of
// it is recorded, even where it looks like one of `jp`'s own options.
#[test]
fn a_plugin_command_is_not_walked() {
    let argv: Vec<OsString> = ["prog", "plugin", "--opt=a"]
        .into_iter()
        .map(OsString::from)
        .collect();
    let mut cmd = command();
    let matches = cmd.try_get_matches_from_mut(&argv).unwrap();

    let spelling = Spelling::of(&cmd, &matches, &argv);

    assert!(spelling.attached.is_empty());
}

/// `argv` (after the program name) with each from-end value attached to its
/// option, its words joined by a space, or `None` when it holds none to attach.
fn from_end_attached(argv: &[&str]) -> Option<String> {
    let argv: Vec<OsString> = ["prog"].iter().chain(argv).map(OsString::from).collect();

    let words = attach_from_end_values(&command(), &argv)?;
    let words: Vec<_> = words.iter().skip(1).map(|w| w.to_string_lossy()).collect();

    Some(words.join(" "))
}

#[test]
fn a_from_end_value_is_attached_to_the_option_before_it() {
    assert_eq!(
        from_end_attached(&["sub", "--opt", "-3..-1"]).as_deref(),
        Some("sub --opt=-3..-1")
    );
    assert_eq!(
        from_end_attached(&["sub", "-o", "-3,-1", "words"]).as_deref(),
        Some("sub -o=-3,-1 words")
    );
    assert_eq!(
        from_end_attached(&["sub", "--option", "-3"]).as_deref(),
        Some("sub --option=-3")
    );
}

#[test]
fn a_from_end_value_is_attached_to_the_option_that_ends_a_short_cluster() {
    assert_eq!(
        from_end_attached(&["sub", "-xo", "-3.."]).as_deref(),
        Some("sub -xo=-3..")
    );
}

#[test]
fn every_from_end_value_is_attached() {
    assert_eq!(
        from_end_attached(&["--cfg", "-1", "sub", "--opt", "-2..", "--opt", "-3"]).as_deref(),
        Some("--cfg=-1 sub --opt=-2.. --opt=-3")
    );
}

// Any other word that starts with `-` may be a flag, known or mistyped, and is
// clap's to read.
#[test]
fn only_a_word_that_starts_with_a_dash_and_a_digit_is_attached() {
    for word in ["-x", "-y3", "--3", "-", "-.3", "3..-1", "words"] {
        assert_eq!(
            from_end_attached(&["sub", "--opt", word]),
            None,
            "attached {word:?}"
        );
    }
}

#[test]
fn an_option_that_has_its_value_takes_no_from_end_value() {
    assert_eq!(from_end_attached(&["sub", "--opt=a", "-3"]), None);
    assert_eq!(from_end_attached(&["sub", "-oa", "-3"]), None);
    assert_eq!(from_end_attached(&["sub", "-o=a", "-3"]), None);
    assert_eq!(from_end_attached(&["sub", "--opt", "a", "-3"]), None);
}

#[test]
fn a_from_end_value_after_anything_but_an_open_option_is_kept() {
    // A flag that takes no value.
    assert_eq!(from_end_attached(&["sub", "-x", "-3"]), None);
    // An option the command does not declare.
    assert_eq!(from_end_attached(&["sub", "--nope", "-3"]), None);
    // A positional word.
    assert_eq!(from_end_attached(&["sub", "words", "-3"]), None);
    // An option of another command: `--opt` is declared on `sub` alone.
    assert_eq!(from_end_attached(&["--opt", "-3", "sub"]), None);
}

#[test]
fn nothing_after_a_double_dash_is_attached() {
    assert_eq!(from_end_attached(&["sub", "--opt", "--", "-3"]), None);
    assert_eq!(from_end_attached(&["sub", "--", "--opt", "-3"]), None);

    // A value before it still is, and the words after it are kept as written.
    assert_eq!(
        from_end_attached(&["sub", "--opt", "-3", "--", "--opt", "-4"]).as_deref(),
        Some("sub --opt=-3 -- --opt -4")
    );
}

// `--cfg` takes the next argument even when it names a subcommand, so the walk
// must not descend into `sub` there.
#[test]
fn the_walk_does_not_descend_into_a_value_that_names_a_subcommand() {
    assert_eq!(from_end_attached(&["--cfg", "sub", "--opt", "-3"]), None);
    assert_eq!(
        from_end_attached(&["--cfg", "sub", "s", "--opt", "-3"]).as_deref(),
        Some("--cfg sub s --opt=-3")
    );
}

// `--raw` takes `--opt` as its value, so `--opt` is no option there and has no
// value to take.
#[test]
fn an_option_read_as_another_options_value_takes_no_from_end_value() {
    assert_eq!(from_end_attached(&["sub", "--raw", "--opt", "-3"]), None);
    assert_eq!(
        from_end_attached(&["sub", "--raw", "--opt", "--opt", "-3"]).as_deref(),
        Some("sub --raw --opt --opt=-3")
    );
}

// Everything after a plugin's command name is the plugin's to read.
#[test]
fn a_plugin_commands_arguments_are_kept_as_written() {
    assert_eq!(from_end_attached(&["plugin", "--cfg", "-3"]), None);
    assert_eq!(
        from_end_attached(&["--cfg", "-3", "plugin", "--cfg", "-4"]).as_deref(),
        Some("--cfg=-3 plugin --cfg -4")
    );
}

// A count the walk disagrees with is answered with every value attached, which
// keeps every value the option's own.
#[test]
fn a_mismatched_count_reads_every_value_as_attached() {
    let mut spelling = Spelling::default();
    spelling
        .attached
        .insert("opt".to_owned(), vec![false, false]);

    assert_eq!(spelling.attached("opt", 3), [true, true, true]);
    assert_eq!(spelling.attached("opt", 2), [false, false]);
    assert_eq!(Spelling::default().attached("opt", 1), [true]);
}
