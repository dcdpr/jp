use super::*;

/// Declining the picker is a choice, not a broken run.
/// The message says so plainly and omits the keyword help, which the user just
/// used correctly to reach the picker in the first place.
#[test]
fn no_conversation_selected_is_short_and_expected() {
    let output = Error::from(crate::error::Error::NoConversationSelected);

    assert_eq!(output.message.as_deref(), Some("No conversation selected."));
    assert!(
        output.expected,
        "declining the picker must not trigger failure-only diagnostics"
    );
}

/// The keyword help printed alongside describes the grammar of the command that
/// failed.
/// A command that rejects session keywords must not be told to use them, and
/// one with no `--new` must not be pointed at it.
#[test]
fn no_conversation_target_help_follows_the_grammar() {
    let output = Error::from(crate::error::Error::NoConversationTarget {
        session: false,
        multi: false,
        allow_new: false,
    });
    let message = output.message.expect("a message");

    assert!(
        !message.contains("session"),
        "session keywords advertised to a command that rejects them: {message}"
    );
    assert!(
        !message.contains("+l, +live"),
        "multi-target keywords advertised to a single-target command: {message}"
    );
    assert!(
        !message.contains("--new"),
        "--new suggested to a command that has no such flag: {message}"
    );
    assert!(
        !output.expected,
        "an unresolvable target is a failure, unlike a declined picker"
    );
}

#[test]
fn no_conversation_target_help_includes_what_the_command_accepts() {
    let output = Error::from(crate::error::Error::NoConversationTarget {
        session: true,
        multi: true,
        allow_new: true,
    });
    let message = output.message.expect("a message");

    assert!(message.contains("s, session"), "missing session row");
    assert!(message.contains("+l, +live"), "missing multi-target row");
    assert!(message.contains("--new"), "missing --new suggestion");
}
