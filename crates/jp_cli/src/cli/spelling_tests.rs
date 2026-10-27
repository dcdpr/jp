use clap::{Arg, ArgAction};

use super::*;

/// A command with an optional-value option `--opt` / `-o`, a flag `-x`, a root
/// option `--cfg` that requires a value, and a subcommand `sub` taking words.
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
