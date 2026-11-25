use super::*;

fn parse(args: &[&str]) -> Cli {
    Cli::try_parse_from(iter_argv(args)).unwrap()
}

fn iter_argv<'a>(args: &'a [&'a str]) -> impl Iterator<Item = &'a str> {
    ["jp"].into_iter().chain(args.iter().copied())
}

#[test]
fn persistence_is_on_unless_turned_off() {
    assert!(parse(&["conversation", "ls"]).globals.persist);
    assert!(
        !parse(&["--no-persist", "conversation", "ls"])
            .globals
            .persist
    );
    assert!(!parse(&["-P", "conversation", "ls"]).globals.persist);
}

#[test]
fn the_default_globals_are_those_of_a_command_with_no_global_flags() {
    assert_eq!(
        format!("{:?}", Globals::default()),
        format!("{:?}", parse(&["conversation", "ls"]).globals)
    );
}

// Global options are accepted on either side of the subcommand.
#[test]
fn a_global_option_after_the_subcommand_is_read() {
    let cli = parse(&["conversation", "ls", "-vv", "--cfg", "a=b", "--format=json"]);

    assert_eq!(cli.globals.verbose, 2);
    assert_eq!(cli.globals.config.len(), 1);
    assert_eq!(cli.globals.format, CliFormat::Json);
}

#[test]
fn unset_options_take_their_defaults() {
    let cli = parse(&["conversation", "ls"]);

    assert_eq!(cli.globals.format, CliFormat::Auto);
    assert!(matches!(cli.globals.log_format, LogFormat::Auto));
    assert_eq!(cli.globals.verbose, 0);
    assert!(!cli.globals.quiet);
    assert!(cli.globals.config.is_empty());
    assert_eq!(cli.root.threads, None);
}

#[test]
fn threads_is_only_accepted_before_the_subcommand() {
    assert_eq!(
        parse(&["--threads", "2", "conversation", "ls"])
            .root
            .threads
            .map(NonZeroUsize::get),
        Some(2)
    );
    assert!(Cli::try_parse_from(iter_argv(&["conversation", "ls", "--threads", "2"])).is_err());
}

#[test]
fn an_alias_selects_its_subcommand() {
    assert!(matches!(parse(&["q", "hello"]).command, Commands::Query(_)));
    assert!(matches!(
        parse(&["conversations", "ls"]).command,
        Commands::Conversation(_)
    ));
}

#[test]
fn an_unknown_subcommand_is_a_plugin_command_with_its_arguments() {
    let Commands::External(args) = parse(&["serve", "web", "--port", "8080"]).command else {
        panic!("an unknown subcommand is a plugin's");
    };

    assert_eq!(args, ["serve", "web", "--port", "8080"]);
}

#[test]
fn no_subcommand_is_an_error() {
    let Err(error) = Cli::try_parse_from(["jp"]) else {
        panic!("`jp` alone must not parse");
    };

    // A bare `jp` prints help rather than complaining about a missing command.
    assert_eq!(
        error.kind(),
        clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
    );
}

/// The message `jp <args>` is rejected with.
fn rejection(args: &[&str]) -> String {
    let Err(error) = Cli::try_parse_from(iter_argv(args)) else {
        panic!("`jp {}` must not parse", args.join(" "));
    };

    error.render().to_string()
}

// The shell splits the command line into words, and each word is read whole:
// `-n-tfoo` is the flag `-n` followed by an unknown flag `--`, never `-n` and
// `-tfoo`.
#[test]
fn a_dash_inside_a_word_does_not_start_another_flag() {
    assert!(Cli::try_parse_from(iter_argv(&["query", "-n", "-tfoo"])).is_ok());

    assert_eq!(
        rejection(&["query", "-n-tfoo"]),
        "error: unexpected argument '--' found\n\n  tip: to pass '--' as a value, use '-- \
         --'\n\nUsage: jp query [OPTIONS] [QUERY]...\n\nFor more information, try '--help'.\n"
    );
}

// A word that starts with `-` and a digit is a value, never a flag, so it is
// its option's whether or not `=` joins the two.
#[test]
fn a_from_end_value_is_its_options_as_a_word_of_its_own() {
    for args in [
        &["conversation", "compact", "--reset", "-3..-1"][..],
        &["conversation", "compact", "--reset=-3..-1"][..],
        &["conversation", "print", "--turn", "-3,-1"][..],
        &[
            "conversation",
            "print",
            "--turn",
            "-3..",
            "--keep-last",
            "-2",
        ][..],
    ] {
        assert!(
            Cli::try_parse_from(iter_argv(args)).is_ok(),
            "`jp {}` should parse",
            args.join(" ")
        );
    }
}

// The value is its option's for good: one the option cannot read is reported as
// that option's value, not as an unknown flag.
#[test]
fn a_from_end_value_its_option_rejects_reports_the_options_error() {
    assert_eq!(
        rejection(&["conversation", "compact", "--reset", "-3..x"]),
        "error: invalid value '-3..x' for '--reset [<COMPACTIONS>]': invalid compaction number \
         `x`\n\nFor more information, try '--help'.\n"
    );
}

// After a flag that takes no value the word has no option to belong to, and
// stays the unknown flag clap reads it as.
#[test]
fn a_from_end_value_after_a_flag_without_a_value_is_an_unknown_flag() {
    assert_eq!(
        rejection(&["conversation", "compact", "--dry-run", "-3..-1"]),
        "error: unexpected argument '-3' found\n\n  tip: to pass '-3' as a value, use '-- \
         -3'\n\nUsage: jp conversation compact [OPTIONS] [id]...\n\nFor more information, try \
         '--help'.\n"
    );
}

/// Append each short flag of `cmd` and its subcommands that is a digit, as
/// `<command path> -<digit>`.
fn digit_shorts(cmd: &Command, path: &str, found: &mut Vec<String>) {
    for arg in cmd.get_arguments() {
        let aliases = arg.get_all_short_aliases().unwrap_or_default();
        for short in arg.get_short().into_iter().chain(aliases) {
            if short.is_ascii_digit() {
                found.push(format!("{path} -{short}"));
            }
        }
    }

    for sub in cmd.get_subcommands() {
        digit_shorts(sub, &format!("{path} {}", sub.get_name()), found);
    }
}

// Reading a word that starts with `-` and a digit as a value is only right
// while no such word can be a flag.
#[test]
fn no_flag_starts_with_a_digit() {
    let mut cmd = Cli::command();
    cmd.build();

    let mut found = vec![];
    digit_shorts(&cmd, "jp", &mut found);

    assert_eq!(found, Vec::<String>::new());
}

// A plugin reads its own arguments, so none of them is rewritten.
#[test]
fn a_plugin_commands_from_end_value_is_left_as_written() {
    let Commands::External(args) = parse(&["serve", "web", "--port", "-3..-1"]).command else {
        panic!("an unknown subcommand is a plugin's");
    };

    assert_eq!(args, ["serve", "web", "--port", "-3..-1"]);
}

// An error raised while reading a subcommand's arguments back gets the same
// usage line as one clap raises while matching them: the subcommand's own.
#[test]
fn an_error_reading_arguments_back_carries_the_usage_line() {
    let Err(error) = Cli::try_parse_from(["jp", "query", "-i", "+pinned"]) else {
        panic!("a multi-target keyword must be refused on a single-target command");
    };

    assert_eq!(
        error.render().to_string(),
        "error: multi-target keyword '+pinned' is not supported by this command\n\n\nUsage: jp \
         query [OPTIONS] [QUERY]...\n\nFor more information, try '--help'.\n"
    );
}

// A value the flag's parser rejects reports the parser's own error, naming the
// flag and the value, as clap does for any argument.
#[test]
fn a_value_the_flag_rejects_reports_the_parsers_error() {
    let Err(error) = Cli::try_parse_from(["jp", "query", "-tfoo,,bar"]) else {
        panic!("an empty tool name must be refused");
    };

    assert_eq!(
        error.render().to_string(),
        "error: invalid value 'foo,,bar' for '--tool [<tools>]': empty tool name in \
         'foo,,bar'\n\nFor more information, try '--help'.\n"
    );
}
