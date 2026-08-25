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
#[command(name = "test-positional-session-single")]
struct TestPositionalSessionSingle {
    #[command(flatten)]
    target: PositionalIds<true, false>,
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
    assert!(cmd.target.ids().is_empty());
}

#[test]
fn positional_multi_one_keyword() {
    let cmd = TestPositionalMulti::try_parse_from(["test-positional-multi", "recent"]).unwrap();
    assert_eq!(cmd.target.ids(), &[ConversationTarget::Recent]);
}

#[test]
fn positional_multi_session_keyword() {
    let cmd = TestPositionalMulti::try_parse_from(["test-positional-multi", "+session"]).unwrap();
    assert_eq!(cmd.target.ids(), &[ConversationTarget::AllSession]);
}

#[test]
fn positional_multi_stdin_dash() {
    let cmd = TestPositionalMulti::try_parse_from(["test-positional-multi", "-"]).unwrap();
    assert_eq!(cmd.target.ids(), &[ConversationTarget::Stdin]);
}

#[test]
fn positional_multi_rejects_stdin_with_id() {
    let err =
        TestPositionalMulti::try_parse_from(["test-positional-multi", "-", "jp-c17000000000"]);
    assert!(err.is_err());
}

#[test]
fn positional_single_rejects_stdin() {
    let err = TestPositionalSingle::try_parse_from(["test-positional-single", "-"]);
    assert!(err.is_err());
}

#[test]
fn positional_multi_multiple_ids() {
    let cmd = TestPositionalMulti::try_parse_from([
        "test-positional-multi",
        "jp-c17000000000",
        "jp-c17000000001",
    ])
    .unwrap();
    assert_eq!(cmd.target.ids().len(), 2);
    assert!(matches!(cmd.target.ids()[0], ConversationTarget::Id(_)));
    assert!(matches!(cmd.target.ids()[1], ConversationTarget::Id(_)));
}

#[test]
fn positional_multi_rejects_keyword_in_multi() {
    let err =
        TestPositionalMulti::try_parse_from(["test-positional-multi", "recent", "jp-c17000000000"]);
    assert!(err.is_err());
}

#[test]
fn positional_single_no_args() {
    let cmd = TestPositionalSingle::try_parse_from(["test-positional-single"]).unwrap();
    assert!(cmd.target.ids().is_empty());
}

#[test]
fn positional_single_one_keyword() {
    let cmd = TestPositionalSingle::try_parse_from(["test-positional-single", "recent"]).unwrap();
    assert_eq!(cmd.target.ids(), &[ConversationTarget::Recent]);
}

#[test]
fn positional_single_rejects_session() {
    let err = TestPositionalSingle::try_parse_from(["test-positional-single", "+session"]);
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
    assert!(cmd.target.ids().is_empty());
}

#[test]
fn flag_multi_bare_flag_is_picker() {
    let cmd = TestFlagMulti::try_parse_from(["test-flag-multi", "--id"]).unwrap();
    assert_eq!(cmd.target.ids(), &[ConversationTarget::Picker(
        PickerFilter::default()
    )]);
}

#[test]
fn flag_multi_keyword() {
    let cmd = TestFlagMulti::try_parse_from(["test-flag-multi", "--id", "recent"]).unwrap();
    assert_eq!(cmd.target.ids(), &[ConversationTarget::Recent]);
}

#[test]
fn flag_multi_short_flag() {
    let cmd = TestFlagMulti::try_parse_from(["test-flag-multi", "-i", "session"]).unwrap();
    assert_eq!(cmd.target.ids(), &[ConversationTarget::SessionPrevious]);
}

#[test]
fn flag_multi_comma_separated() {
    let cmd = TestFlagMulti::try_parse_from([
        "test-flag-multi",
        "--id",
        "jp-c17000000000,jp-c17000000001",
    ])
    .unwrap();
    assert_eq!(cmd.target.ids().len(), 2);
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
    assert_eq!(cmd.target.ids().len(), 2);
}

/// Free text parses to a fuzzy `Picker`, which has no keyword name — so the
/// keyword check waved it through, and `resolve_targets` then took the
/// fallback-chain branch on a list containing a literal ID, tripping its
/// `debug_assert` (and silently dropping the ID in release).
#[test]
fn flag_multi_rejects_fuzzy_text_mixed_with_id() {
    let err =
        TestFlagMulti::try_parse_from(["test-flag-multi", "--id", "some title,jp-c17000000000"]);
    assert!(err.is_err());
}

#[test]
fn flag_multi_rejects_picker_mixed_with_id() {
    let err = TestFlagMulti::try_parse_from(["test-flag-multi", "--id", "?,jp-c17000000000"]);
    assert!(err.is_err());
}

#[test]
fn flag_multi_session_keyword() {
    let cmd = TestFlagMulti::try_parse_from(["test-flag-multi", "--id", "+session"]).unwrap();
    assert_eq!(cmd.target.ids(), &[ConversationTarget::AllSession]);
}

#[test]
fn flag_multi_rejects_keyword_in_multi() {
    let err = TestFlagMulti::try_parse_from(["test-flag-multi", "--id", "recent,jp-c17000000000"]);
    assert!(err.is_err());
}

#[test]
fn flag_single_no_flag() {
    let cmd = TestFlagSingle::try_parse_from(["test-flag-single"]).unwrap();
    assert!(cmd.target.ids().is_empty());
}

#[test]
fn flag_single_bare_is_picker() {
    let cmd = TestFlagSingle::try_parse_from(["test-flag-single", "--id"]).unwrap();
    assert_eq!(cmd.target.ids(), &[ConversationTarget::Picker(
        PickerFilter::default()
    )]);
}

#[test]
fn flag_single_keyword() {
    let cmd = TestFlagSingle::try_parse_from(["test-flag-single", "--id", "recent"]).unwrap();
    assert_eq!(cmd.target.ids(), &[ConversationTarget::Recent]);
}

#[test]
fn flag_single_rejects_session() {
    let err = TestFlagSingle::try_parse_from(["test-flag-single", "--id", "+session"]);
    assert!(err.is_err());
}

#[test]
fn keyword_aliases() {
    for (input, expected) in [
        ("r", ConversationTarget::Recent),
        ("recent", ConversationTarget::Recent),
        ("n", ConversationTarget::Newest),
        ("newest", ConversationTarget::Newest),
        ("s", ConversationTarget::SessionPrevious),
        ("session", ConversationTarget::SessionPrevious),
        ("p", ConversationTarget::RecentPinned),
        ("pinned", ConversationTarget::RecentPinned),
        ("+session", ConversationTarget::AllSession),
        ("+s", ConversationTarget::AllSession),
        ("+pinned", ConversationTarget::AllPinned),
        ("+p", ConversationTarget::AllPinned),
        (".", ConversationTarget::SessionActive),
        ("active", ConversationTarget::SessionActive),
        ("+l", ConversationTarget::AllLive),
        ("+live", ConversationTarget::AllLive),
        (
            "?p",
            ConversationTarget::Picker(PickerFilter {
                pinned: true,
                ..PickerFilter::default()
            }),
        ),
        (
            "?pinned",
            ConversationTarget::Picker(PickerFilter {
                pinned: true,
                ..PickerFilter::default()
            }),
        ),
        (
            "?s",
            ConversationTarget::Picker(PickerFilter {
                session: true,
                ..PickerFilter::default()
            }),
        ),
        (
            "?session",
            ConversationTarget::Picker(PickerFilter {
                session: true,
                ..PickerFilter::default()
            }),
        ),
    ] {
        let cmd = TestPositionalMulti::try_parse_from(["test-positional-multi", input]).unwrap();
        assert_eq!(cmd.target.ids(), &[expected], "failed for input: {input}");
    }
}

#[test]
fn positional_session_single_accepts_session_previous() {
    let cmd = TestPositionalSessionSingle::try_parse_from(["test-positional-session-single", "s"])
        .unwrap();
    assert_eq!(cmd.target.ids(), &[ConversationTarget::SessionPrevious]);
}

#[test]
fn positional_session_single_rejects_multi_target_session() {
    let err =
        TestPositionalSessionSingle::try_parse_from(["test-positional-session-single", "+session"]);
    assert!(err.is_err());
}

#[test]
fn positional_session_single_rejects_multi_target_pinned() {
    let err =
        TestPositionalSessionSingle::try_parse_from(["test-positional-session-single", "+pinned"]);
    assert!(err.is_err());
}

#[test]
fn positional_session_single_rejects_multi_target_live() {
    let err = TestPositionalSessionSingle::try_parse_from(["test-positional-session-single", "+l"]);
    assert!(err.is_err());
}

/// `jp c use +archived` reached `run_unarchive`, which takes the *first* of
/// however many IDs the keyword resolved to — silently unarchiving and
/// activating an arbitrary conversation.
/// A single-target command must reject the keyword outright.
#[test]
fn positional_session_single_rejects_multi_target_archived() {
    let err = TestPositionalSessionSingle::try_parse_from(["test-positional-session-single", "+a"]);
    assert!(err.is_err());
}

/// The single-target archive keywords stay accepted; only the multi-target one
/// is rejected.
#[test]
fn positional_session_single_accepts_single_archive_keywords() {
    for (input, expected) in [
        ("a", ConversationTarget::Archived),
        (
            "?a",
            ConversationTarget::Picker(PickerFilter {
                archived: true,
                ..PickerFilter::default()
            }),
        ),
    ] {
        let cmd =
            TestPositionalSessionSingle::try_parse_from(["test-positional-session-single", input])
                .unwrap();
        assert_eq!(cmd.target.ids(), &[expected], "failed for input: {input}");
    }
}

#[test]
fn positional_session_single_accepts_active() {
    let cmd = TestPositionalSessionSingle::try_parse_from(["test-positional-session-single", "."])
        .unwrap();
    assert_eq!(cmd.target.ids(), &[ConversationTarget::SessionActive]);
}

#[test]
fn positional_single_rejects_active_without_session_support() {
    let err = TestPositionalSingle::try_parse_from(["test-positional-single", "active"]);
    assert!(err.is_err());
}

#[test]
fn flag_multi_accepts_all_live_and_active() {
    let cmd = TestFlagMulti::try_parse_from(["test-flag-multi", "--id", "+l"]).unwrap();
    assert_eq!(cmd.target.ids(), &[ConversationTarget::AllLive]);

    let cmd = TestFlagMulti::try_parse_from(["test-flag-multi", "-i", "."]).unwrap();
    assert_eq!(cmd.target.ids(), &[ConversationTarget::SessionActive]);
}

/// `--id` takes an optional value, which changes how clap binds a value to it.
/// All three spellings must reach the same target.
#[test]
fn flag_multi_value_attaches_to_short_and_long_forms() {
    for args in [
        ["test-flag-multi", "-i."].as_slice(),
        ["test-flag-multi", "-i", "."].as_slice(),
        ["test-flag-multi", "--id=."].as_slice(),
        ["test-flag-multi", "--id", "."].as_slice(),
    ] {
        let cmd = TestFlagMulti::try_parse_from(args).unwrap();
        assert_eq!(
            cmd.target.ids(),
            &[ConversationTarget::SessionActive],
            "failed for args: {args:?}"
        );
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
    assert!(
        !long.contains("session"),
        "long_help should not mention session: {long}"
    );
}

/// The keyword table is user-visible output — it lands in `--help` and in the
/// hint printed when a command resolves no target.
/// Pinned exactly so a new row can't silently break the column alignment.
#[test]
fn target_help_table() {
    insta::assert_snapshot!(format_target_help(true, true, false), @r#"
    Conversation Targeting

    Use a conversation ID (e.g. jp-c17761673600), a keyword, or any text to
    fuzzy-search by title.

    Interactive Filter/Picker:
      ?                             select from all
      ?p, ?pinned                   select from pinned
      ?s, ?session                  select from session

    Conversation Aliases:
      ., active                     target the session's active conversation
      r, recent                     target most recently activated in workspace
      n, newest                     target newest created
      p, pinned                     target most recently pinned
      s, session                    target the session's previous conversation

    Multi-Target Keywords:
      +l, +live                     target all live conversations
      +p, +pinned                   target all pinned
      +s, +session                  target all activated in session
      -                             read IDs from stdin, one per line
    "#);
}

#[test]
fn help_text_lists_active_and_all_live_keywords() {
    let cmd = TestPositionalMulti::command();
    let arg = cmd.get_arguments().find(|a| a.get_id() == "id").unwrap();
    let long = arg.get_long_help().unwrap().to_string();
    assert!(long.contains("., active"), "missing active row: {long}");
    assert!(long.contains("+l, +live"), "missing live row: {long}");
}

/// The active keyword needs session state to resolve, so a command that doesn't
/// support session targets must not advertise it.
#[test]
fn help_text_without_session_omits_active_keyword() {
    let cmd = TestPositionalSingle::command();
    let arg = cmd.get_arguments().find(|a| a.get_id() == "id").unwrap();
    let long = arg.get_long_help().unwrap().to_string();
    // Matched on the row prefix, not the bare word: the "Interactive
    // Filter/Picker" heading also ends in "active".
    assert!(
        !long.contains("., active"),
        "should omit active row: {long}"
    );
}

#[test]
fn help_text_multi_shows_multi_target_section() {
    let cmd = TestPositionalMulti::command();
    let arg = cmd.get_arguments().find(|a| a.get_id() == "id").unwrap();
    let long = arg.get_long_help().unwrap().to_string();
    assert!(
        long.contains("Multi-Target Keywords"),
        "long_help should contain multi-target section: {long}"
    );
}

#[test]
fn help_text_single_omits_multi_target_section() {
    let cmd = TestPositionalSessionSingle::command();
    let arg = cmd.get_arguments().find(|a| a.get_id() == "id").unwrap();
    let long = arg.get_long_help().unwrap().to_string();
    assert!(
        !long.contains("Multi-Target"),
        "long_help should not contain multi-target section: {long}"
    );
}
