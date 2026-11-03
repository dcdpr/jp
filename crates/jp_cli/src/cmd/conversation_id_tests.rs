use super::*;
use crate::{cli::parse_args, cmd::target::PickerFilter};

type PositionalMulti = PositionalIds<true, true>;
type PositionalSingle = PositionalIds<false, false>;
type PositionalSessionSingle = PositionalIds<true, false>;
type FlagMulti = FlagIds<true, true>;
type FlagSingle = FlagIds<false, false>;

#[test]
fn positional_multi_no_args() {
    let cmd = parse_args::<PositionalMulti>(&[]).unwrap();
    assert!(cmd.ids().is_empty());
}

#[test]
fn positional_multi_one_keyword() {
    let cmd = parse_args::<PositionalMulti>(&["recent"]).unwrap();
    assert_eq!(cmd.ids(), &[ConversationTarget::Recent]);
}

#[test]
fn positional_multi_session_keyword() {
    let cmd = parse_args::<PositionalMulti>(&["+session"]).unwrap();
    assert_eq!(cmd.ids(), &[ConversationTarget::AllSession]);
}

#[test]
fn positional_multi_stdin_dash() {
    let cmd = parse_args::<PositionalMulti>(&["-"]).unwrap();
    assert_eq!(cmd.ids(), &[ConversationTarget::Stdin]);
}

#[test]
fn positional_multi_rejects_stdin_with_id() {
    let err = parse_args::<PositionalMulti>(&["-", "jp-c17000000000"]);
    assert!(err.is_err());
}

#[test]
fn positional_single_rejects_stdin() {
    let err = parse_args::<PositionalSingle>(&["-"]);
    assert!(err.is_err());
}

#[test]
fn positional_multi_multiple_ids() {
    let cmd = parse_args::<PositionalMulti>(&["jp-c17000000000", "jp-c17000000001"]).unwrap();
    assert_eq!(cmd.ids().len(), 2);
    assert!(matches!(cmd.ids()[0], ConversationTarget::Id(_)));
    assert!(matches!(cmd.ids()[1], ConversationTarget::Id(_)));
}

#[test]
fn positional_multi_rejects_keyword_in_multi() {
    let err = parse_args::<PositionalMulti>(&["recent", "jp-c17000000000"]);
    assert!(err.is_err());
}

#[test]
fn positional_single_no_args() {
    let cmd = parse_args::<PositionalSingle>(&[]).unwrap();
    assert!(cmd.ids().is_empty());
}

#[test]
fn positional_single_one_keyword() {
    let cmd = parse_args::<PositionalSingle>(&["recent"]).unwrap();
    assert_eq!(cmd.ids(), &[ConversationTarget::Recent]);
}

#[test]
fn positional_single_rejects_session() {
    let err = parse_args::<PositionalSingle>(&["+session"]);
    assert!(err.is_err());
}

#[test]
fn positional_single_rejects_two_values() {
    let err = parse_args::<PositionalSingle>(&["jp-c17000000000", "jp-c17000000001"]);
    assert!(err.is_err());
}

#[test]
fn flag_multi_no_flag() {
    let cmd = parse_args::<FlagMulti>(&[]).unwrap();
    assert!(cmd.ids().is_empty());
}

#[test]
fn flag_multi_bare_flag_is_picker() {
    let cmd = parse_args::<FlagMulti>(&["--id"]).unwrap();
    assert_eq!(cmd.ids(), &[ConversationTarget::Picker(
        PickerFilter::default()
    )]);
}

#[test]
fn flag_multi_keyword() {
    let cmd = parse_args::<FlagMulti>(&["--id", "recent"]).unwrap();
    assert_eq!(cmd.ids(), &[ConversationTarget::Recent]);
}

#[test]
fn flag_multi_short_flag() {
    let cmd = parse_args::<FlagMulti>(&["-i", "session"]).unwrap();
    assert_eq!(cmd.ids(), &[ConversationTarget::SessionPrevious]);
}

#[test]
fn flag_multi_comma_separated() {
    let cmd = parse_args::<FlagMulti>(&["--id", "jp-c17000000000,jp-c17000000001"]).unwrap();
    assert_eq!(cmd.ids().len(), 2);
}

#[test]
fn flag_multi_repeated() {
    let cmd =
        parse_args::<FlagMulti>(&["--id", "jp-c17000000000", "--id", "jp-c17000000001"]).unwrap();
    assert_eq!(cmd.ids().len(), 2);
}

/// Free text parses to a fuzzy `Picker`, which has no keyword name — so the
/// keyword check waved it through, and `resolve_targets` then took the
/// fallback-chain branch on a list containing a literal ID, tripping its
/// `debug_assert` (and silently dropping the ID in release).
#[test]
fn flag_multi_rejects_fuzzy_text_mixed_with_id() {
    let err = parse_args::<FlagMulti>(&["--id", "some title,jp-c17000000000"]);
    assert!(err.is_err());
}

#[test]
fn flag_multi_rejects_picker_mixed_with_id() {
    let err = parse_args::<FlagMulti>(&["--id", "?,jp-c17000000000"]);
    assert!(err.is_err());
}

#[test]
fn flag_multi_session_keyword() {
    let cmd = parse_args::<FlagMulti>(&["--id", "+session"]).unwrap();
    assert_eq!(cmd.ids(), &[ConversationTarget::AllSession]);
}

#[test]
fn flag_multi_rejects_keyword_in_multi() {
    let err = parse_args::<FlagMulti>(&["--id", "recent,jp-c17000000000"]);
    assert!(err.is_err());
}

#[test]
fn flag_single_no_flag() {
    let cmd = parse_args::<FlagSingle>(&[]).unwrap();
    assert!(cmd.ids().is_empty());
}

#[test]
fn flag_single_bare_is_picker() {
    let cmd = parse_args::<FlagSingle>(&["--id"]).unwrap();
    assert_eq!(cmd.ids(), &[ConversationTarget::Picker(
        PickerFilter::default()
    )]);
}

#[test]
fn flag_single_keyword() {
    let cmd = parse_args::<FlagSingle>(&["--id", "recent"]).unwrap();
    assert_eq!(cmd.ids(), &[ConversationTarget::Recent]);
}

#[test]
fn flag_single_rejects_session() {
    let err = parse_args::<FlagSingle>(&["--id", "+session"]);
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
        let cmd = parse_args::<PositionalMulti>(&[input]).unwrap();
        assert_eq!(cmd.ids(), &[expected], "failed for input: {input}");
    }
}

#[test]
fn positional_session_single_accepts_session_previous() {
    let cmd = parse_args::<PositionalSessionSingle>(&["s"]).unwrap();
    assert_eq!(cmd.ids(), &[ConversationTarget::SessionPrevious]);
}

#[test]
fn positional_session_single_rejects_multi_target_session() {
    let err = parse_args::<PositionalSessionSingle>(&["+session"]);
    assert!(err.is_err());
}

#[test]
fn positional_session_single_rejects_multi_target_pinned() {
    let err = parse_args::<PositionalSessionSingle>(&["+pinned"]);
    assert!(err.is_err());
}

#[test]
fn positional_session_single_rejects_multi_target_live() {
    let err = parse_args::<PositionalSessionSingle>(&["+l"]);
    assert!(err.is_err());
}

/// `jp c use +archived` reached `run_unarchive`, which takes the *first* of
/// however many IDs the keyword resolved to — silently unarchiving and
/// activating an arbitrary conversation.
/// A single-target command must reject the keyword outright.
#[test]
fn positional_session_single_rejects_multi_target_archived() {
    let err = parse_args::<PositionalSessionSingle>(&["+a"]);
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
        let cmd = parse_args::<PositionalSessionSingle>(&[input]).unwrap();
        assert_eq!(cmd.ids(), &[expected], "failed for input: {input}");
    }
}

#[test]
fn positional_session_single_accepts_active() {
    let cmd = parse_args::<PositionalSessionSingle>(&["."]).unwrap();
    assert_eq!(cmd.ids(), &[ConversationTarget::SessionActive]);
}

#[test]
fn positional_single_rejects_active_without_session_support() {
    let err = parse_args::<PositionalSingle>(&["active"]);
    assert!(err.is_err());
}

#[test]
fn flag_multi_accepts_all_live_and_active() {
    let cmd = parse_args::<FlagMulti>(&["--id", "+l"]).unwrap();
    assert_eq!(cmd.ids(), &[ConversationTarget::AllLive]);

    let cmd = parse_args::<FlagMulti>(&["-i", "."]).unwrap();
    assert_eq!(cmd.ids(), &[ConversationTarget::SessionActive]);
}

/// `--id` takes an optional value, which changes how clap binds a value to it.
/// All three spellings must reach the same target.
#[test]
fn flag_multi_value_attaches_to_short_and_long_forms() {
    for args in [
        ["-i."].as_slice(),
        ["-i", "."].as_slice(),
        ["--id=."].as_slice(),
        ["--id", "."].as_slice(),
    ] {
        let cmd = parse_args::<FlagMulti>(args).unwrap();
        assert_eq!(
            cmd.ids(),
            &[ConversationTarget::SessionActive],
            "failed for args: {args:?}"
        );
    }
}

#[test]
fn help_text_with_session_mentions_session() {
    let cmd = PositionalMulti::augment(Command::new("test"));
    let arg = cmd.get_arguments().find(|a| a.get_id() == "id").unwrap();
    let long = arg.get_long_help().unwrap().to_string();
    assert!(long.contains("session"), "long_help should mention session");
}

#[test]
fn help_text_without_session_omits_session_keyword() {
    let cmd = PositionalSingle::augment(Command::new("test"));
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
      ?a, ?archived                 select from archived

    Conversation Aliases:
      ., active                     target the session's active conversation
      r, recent                     target most recently activated in workspace
      n, newest                     target newest created
      p, pinned                     target most recently pinned
      s, session                    target the session's previous conversation
      a, archived                   target most recently archived

    Multi-Target Keywords:
      +l, +live                     target all live conversations
      +p, +pinned                   target all pinned
      +s, +session                  target all activated in session
      +a, +archived                 target all archived
      -                             read IDs from stdin, one per line
    "#);
}

#[test]
fn help_text_lists_active_and_all_live_keywords() {
    let cmd = PositionalMulti::augment(Command::new("test"));
    let arg = cmd.get_arguments().find(|a| a.get_id() == "id").unwrap();
    let long = arg.get_long_help().unwrap().to_string();
    assert!(long.contains("., active"), "missing active row: {long}");
    assert!(long.contains("+l, +live"), "missing live row: {long}");
}

/// The active keyword needs session state to resolve, so a command that doesn't
/// support session targets must not advertise it.
#[test]
fn help_text_without_session_omits_active_keyword() {
    let cmd = PositionalSingle::augment(Command::new("test"));
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
    let cmd = PositionalMulti::augment(Command::new("test"));
    let arg = cmd.get_arguments().find(|a| a.get_id() == "id").unwrap();
    let long = arg.get_long_help().unwrap().to_string();
    assert!(
        long.contains("Multi-Target Keywords"),
        "long_help should contain multi-target section: {long}"
    );
}

#[test]
fn help_text_single_omits_multi_target_section() {
    let cmd = PositionalSessionSingle::augment(Command::new("test"));
    let arg = cmd.get_arguments().find(|a| a.get_id() == "id").unwrap();
    let long = arg.get_long_help().unwrap().to_string();
    assert!(
        !long.contains("Multi-Target"),
        "long_help should not contain multi-target section: {long}"
    );
}
