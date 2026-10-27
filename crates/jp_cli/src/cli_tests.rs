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

// An error raised while reading a subcommand's arguments back gets the same
// usage line as one clap raises while matching them.
#[test]
fn an_error_reading_arguments_back_carries_the_usage_line() {
    let Err(error) = Cli::try_parse_from(["jp", "query", "-tfoo,,bar"]) else {
        panic!("an empty tool name must be refused");
    };

    assert_eq!(
        error.render().to_string(),
        "error: empty tool name in 'foo,,bar'\n\n\nUsage: jp [OPTIONS] <COMMAND>\n\nFor more \
         information, try '--help'.\n"
    );
}
