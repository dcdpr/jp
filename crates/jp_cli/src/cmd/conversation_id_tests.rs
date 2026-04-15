use clap::{CommandFactory, Parser};

use super::*;
use crate::cmd::target::PickerFilter;

// Helper: derive a top-level command that flattens the shared type.
// This is the pattern commands will use.

#[derive(Debug, Parser)]
#[command(name = "test-positional-multi")]
struct TestPositionalMulti {
    #[command(flatten)]
    target: PositionalIds<true, true>,
}

#[derive(Debug, Parser)]
#[command(name = "test-positional-single")]
struct TestPositionalSingle {
    #[command(flatten)]
    target: PositionalIds<false, false>,
}

#[derive(Debug, Parser)]
#[command(name = "test-flag-multi")]
struct TestFlagMulti {
    #[command(flatten)]
    target: FlagIds<true, true>,
}

#[derive(Debug, Parser)]
#[command(name = "test-flag-single")]
struct TestFlagSingle {
    #[command(flatten)]
    target: FlagIds<false, false>,
}

#[test]
fn positional_multi_no_args() {
    let cmd = TestPositionalMulti::try_parse_from(["test-positional-multi"]).unwrap();
    assert!(cmd.target.ids.is_empty());
}

#[test]
fn positional_multi_one_keyword() {
    let cmd = TestPositionalMulti::try_parse_from(["test-positional-multi", "last"]).unwrap();
    assert_eq!(cmd.target.ids, vec![ConversationTarget::LastActivated]);
}

#[test]
fn positional_multi_session_keyword() {
    let cmd = TestPositionalMulti::try_parse_from(["test-positional-multi", "session"]).unwrap();
    assert_eq!(cmd.target.ids, vec![ConversationTarget::Session]);
}

#[test]
fn positional_multi_multiple_ids() {
    let cmd = TestPositionalMulti::try_parse_from([
        "test-positional-multi",
        "jp-c17000000000",
        "jp-c17000000001",
    ])
    .unwrap();
    assert_eq!(cmd.target.ids.len(), 2);
    assert!(matches!(cmd.target.ids[0], ConversationTarget::Id(_)));
    assert!(matches!(cmd.target.ids[1], ConversationTarget::Id(_)));
}

#[test]
fn positional_multi_rejects_keyword_in_multi() {
    let err =
        TestPositionalMulti::try_parse_from(["test-positional-multi", "last", "jp-c17000000000"]);
    assert!(err.is_err());
}

#[test]
fn positional_single_no_args() {
    let cmd = TestPositionalSingle::try_parse_from(["test-positional-single"]).unwrap();
    assert!(cmd.target.ids.is_empty());
}

#[test]
fn positional_single_one_keyword() {
    let cmd = TestPositionalSingle::try_parse_from(["test-positional-single", "last"]).unwrap();
    assert_eq!(cmd.target.ids, vec![ConversationTarget::LastActivated]);
}

#[test]
fn positional_single_rejects_session() {
    let err = TestPositionalSingle::try_parse_from(["test-positional-single", "session"]);
    assert!(err.is_err());
}

#[test]
fn positional_single_rejects_two_values() {
    let err = TestPositionalSingle::try_parse_from([
        "test-positional-single",
        "jp-c17000000000",
        "jp-c17000000001",
    ]);
    assert!(err.is_err());
}

#[test]
fn flag_multi_no_flag() {
    let cmd = TestFlagMulti::try_parse_from(["test-flag-multi"]).unwrap();
    assert!(cmd.target.ids.is_empty());
}

#[test]
fn flag_multi_bare_flag_is_picker() {
    let cmd = TestFlagMulti::try_parse_from(["test-flag-multi", "--id"]).unwrap();
    assert_eq!(cmd.target.ids, vec![ConversationTarget::Picker(
        PickerFilter::default()
    )]);
}

#[test]
fn flag_multi_keyword() {
    let cmd = TestFlagMulti::try_parse_from(["test-flag-multi", "--id", "last"]).unwrap();
    assert_eq!(cmd.target.ids, vec![ConversationTarget::LastActivated]);
}

#[test]
fn flag_multi_short_flag() {
    let cmd = TestFlagMulti::try_parse_from(["test-flag-multi", "-i", "prev"]).unwrap();
    assert_eq!(cmd.target.ids, vec![ConversationTarget::Previous]);
}

#[test]
fn flag_multi_comma_separated() {
    let cmd = TestFlagMulti::try_parse_from([
        "test-flag-multi",
        "--id",
        "jp-c17000000000,jp-c17000000001",
    ])
    .unwrap();
    assert_eq!(cmd.target.ids.len(), 2);
}

#[test]
fn flag_multi_repeated() {
    let cmd = TestFlagMulti::try_parse_from([
        "test-flag-multi",
        "--id",
        "jp-c17000000000",
        "--id",
        "jp-c17000000001",
    ])
    .unwrap();
    assert_eq!(cmd.target.ids.len(), 2);
}

#[test]
fn flag_multi_session_keyword() {
    let cmd = TestFlagMulti::try_parse_from(["test-flag-multi", "--id", "session"]).unwrap();
    assert_eq!(cmd.target.ids, vec![ConversationTarget::Session]);
}

#[test]
fn flag_multi_rejects_keyword_in_multi() {
    let err = TestFlagMulti::try_parse_from(["test-flag-multi", "--id", "last,jp-c17000000000"]);
    assert!(err.is_err());
}

#[test]
fn flag_single_no_flag() {
    let cmd = TestFlagSingle::try_parse_from(["test-flag-single"]).unwrap();
    assert!(cmd.target.ids.is_empty());
}

#[test]
fn flag_single_bare_is_picker() {
    let cmd = TestFlagSingle::try_parse_from(["test-flag-single", "--id"]).unwrap();
    assert_eq!(cmd.target.ids, vec![ConversationTarget::Picker(
        PickerFilter::default()
    )]);
}

#[test]
fn flag_single_keyword() {
    let cmd = TestFlagSingle::try_parse_from(["test-flag-single", "--id", "last"]).unwrap();
    assert_eq!(cmd.target.ids, vec![ConversationTarget::LastActivated]);
}

#[test]
fn flag_single_rejects_session() {
    let err = TestFlagSingle::try_parse_from(["test-flag-single", "--id", "session"]);
    assert!(err.is_err());
}

#[test]
fn keyword_aliases() {
    for (input, expected) in [
        ("l", ConversationTarget::LastActivated),
        ("last", ConversationTarget::LastActivated),
        ("last-active", ConversationTarget::LastActivated),
        ("last-activated", ConversationTarget::LastActivated),
        ("p", ConversationTarget::Previous),
        ("prev", ConversationTarget::Previous),
        ("previous", ConversationTarget::Previous),
        ("c", ConversationTarget::Current),
        ("current", ConversationTarget::Current),
        ("last-created", ConversationTarget::LastCreated),
        ("session", ConversationTarget::Session),
        (
            "pinned",
            ConversationTarget::Picker(PickerFilter { pinned: true }),
        ),
    ] {
        let cmd = TestPositionalMulti::try_parse_from(["test-positional-multi", input]).unwrap();
        assert_eq!(cmd.target.ids, vec![expected], "failed for input: {input}");
    }
}

#[test]
fn help_text_with_session_mentions_session() {
    let cmd = TestPositionalMulti::command();
    let arg = cmd.get_arguments().find(|a| a.get_id() == "id").unwrap();
    let long = arg.get_long_help().unwrap().to_string();
    assert!(long.contains("session"), "long_help should mention session");
}

#[test]
fn help_text_without_session_omits_session_keyword() {
    let cmd = TestPositionalSingle::command();
    let arg = cmd.get_arguments().find(|a| a.get_id() == "id").unwrap();
    let long = arg.get_long_help().unwrap().to_string();
    // The `session` keyword line should not appear, but other mentions of
    // "session" (e.g. "current session's") are fine.
    assert!(
        !long.contains("  session "),
        "long_help should not list the session keyword: {long}"
    );
}
