use clap::CommandFactory as _;

use super::*;

#[test]
fn the_command_line_is_well_formed() {
    Cli::command().debug_assert();
}

#[test]
fn check_reads_changelog_d_unless_given_another_directory() {
    let Cli {
        command: Command::Check { dir },
    } = Cli::try_parse_from(["changelog", "check"]).unwrap();
    assert_eq!(dir.as_str(), "changelog.d");

    let Cli {
        command: Command::Check { dir },
    } = Cli::try_parse_from(["changelog", "check", "docs/changes"]).unwrap();
    assert_eq!(dir.as_str(), "docs/changes");
}
