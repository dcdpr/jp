use clap::Parser as _;

use super::ConfirmFlag;

#[derive(Debug, clap::Parser)]
struct TestCli {
    #[command(flatten)]
    confirm: ConfirmFlag,
}

fn preference(args: &[&str]) -> Option<bool> {
    let mut argv = vec!["test"];
    argv.extend_from_slice(args);
    TestCli::try_parse_from(argv).unwrap().confirm.preference()
}

#[test]
fn no_flag_defers_to_command_default() {
    assert_eq!(preference(&[]), None);
}

#[test]
fn confirm_forces_prompt() {
    assert_eq!(preference(&["--confirm"]), Some(true));
}

#[test]
fn no_confirm_skips_prompt() {
    assert_eq!(preference(&["--no-confirm"]), Some(false));
}

#[test]
fn yes_and_short_are_aliases_for_no_confirm() {
    assert_eq!(preference(&["--yes"]), Some(false));
    assert_eq!(preference(&["-y"]), Some(false));
}

#[test]
fn last_flag_on_the_line_wins() {
    assert_eq!(preference(&["--confirm", "--no-confirm"]), Some(false));
    assert_eq!(preference(&["--no-confirm", "--confirm"]), Some(true));
}
