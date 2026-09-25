use super::*;

#[test]
fn next_inquiry_attempt_increments_per_key_and_resets_per_turn() {
    let mut state = TurnState::default();

    // The first recording for a key is attempt 1; the next for the same key is
    // 2 (a re-ask after an invalid answer, or a reused tool_call_id cycle).
    assert_eq!(state.next_inquiry_attempt("call_1", "confirm"), 1);
    assert_eq!(state.next_inquiry_attempt("call_1", "confirm"), 2);

    // A different question or tool call counts independently.
    assert_eq!(state.next_inquiry_attempt("call_1", "reason"), 1);
    assert_eq!(state.next_inquiry_attempt("call_2", "confirm"), 1);

    // Continuing the first key picks up where it left off rather than
    // restarting (cross-cycle uniqueness within the turn).
    assert_eq!(state.next_inquiry_attempt("call_1", "confirm"), 3);

    // A fresh TurnState (built per turn) resets every counter.
    let mut next_turn = TurnState::default();
    assert_eq!(next_turn.next_inquiry_attempt("call_1", "confirm"), 1);
}

/// A remembered decision covers the tool, not the call it was given for.
#[test]
fn a_remembered_permission_applies_to_every_call_to_the_tool() {
    let mut state = TurnState::default();
    assert_eq!(state.remembered_permission("fs_delete_file"), None);

    state.remember_permission("fs_delete_file", false);

    assert_eq!(state.remembered_permission("fs_delete_file"), Some(false));
    assert_eq!(state.remembered_permission("fs_modify_file"), None);
}

#[test]
fn a_remembered_answer_is_kept_per_tool_and_question() {
    let mut state = TurnState::default();
    state.remember_answer("git_commit", "confirm", Value::Bool(true));

    assert_eq!(
        state.remembered_answer("git_commit", "confirm"),
        Some(&Value::Bool(true))
    );
    assert_eq!(state.remembered_answer("git_commit", "message"), None);
    assert_eq!(state.remembered_answer("git_stage", "confirm"), None);
}
