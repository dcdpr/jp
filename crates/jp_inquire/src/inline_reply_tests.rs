use super::*;

#[test]
fn emacs_keymap_binds_editor_escape() {
    let mut keybindings = default_emacs_keybindings();
    add_custom_bindings(&mut keybindings);

    assert_eq!(
        keybindings.find_binding(KeyModifiers::CONTROL, KeyCode::Char('x')),
        Some(ReedlineEvent::ExecuteHostCommand(
            OPEN_EDITOR_SENTINEL.to_owned()
        ))
    );
}

#[test]
fn emacs_keymap_binds_newline_keys() {
    let mut keybindings = default_emacs_keybindings();
    add_custom_bindings(&mut keybindings);

    let newline = Some(ReedlineEvent::Edit(vec![EditCommand::InsertNewline]));
    assert_eq!(
        keybindings.find_binding(KeyModifiers::SHIFT, KeyCode::Enter),
        newline
    );
    assert_eq!(
        keybindings.find_binding(KeyModifiers::ALT, KeyCode::Enter),
        newline
    );
}

#[test]
fn vi_mode_registers_bindings_into_insert_keymap() {
    // The editor escape is registered into the insert keymap (where typing
    // happens).
    let mut insert = default_vi_insert_keybindings();
    add_custom_bindings(&mut insert);

    assert_eq!(
        insert.find_binding(KeyModifiers::CONTROL, KeyCode::Char('x')),
        Some(ReedlineEvent::ExecuteHostCommand(
            OPEN_EDITOR_SENTINEL.to_owned()
        ))
    );
}

#[test]
fn vi_mode_registers_bindings_into_normal_keymap() {
    // The escape must also work after `Esc` into normal mode, so the custom
    // bindings are registered into the normal keymap too.
    let mut normal = default_vi_normal_keybindings();
    add_custom_bindings(&mut normal);

    assert_eq!(
        normal.find_binding(KeyModifiers::CONTROL, KeyCode::Char('x')),
        Some(ReedlineEvent::ExecuteHostCommand(
            OPEN_EDITOR_SENTINEL.to_owned()
        ))
    );
}

#[test]
fn submit_signal_maps_to_submit() {
    assert_eq!(
        outcome_from_signal(Signal::Success("hello".into()), ""),
        ReplyOutcome::Submit("hello".into())
    );
}

#[test]
fn empty_submit_still_maps_to_submit() {
    // Empty-vs-non-empty is the caller's policy; the widget always submits.
    assert_eq!(
        outcome_from_signal(Signal::Success(String::new()), ""),
        ReplyOutcome::Submit(String::new())
    );
}

#[test]
fn ctrl_c_maps_to_cancelled() {
    assert_eq!(
        outcome_from_signal(Signal::CtrlC, "draft"),
        ReplyOutcome::Cancelled
    );
}

#[test]
fn editor_sentinel_maps_to_open_editor_with_buffer() {
    let outcome = outcome_from_signal(
        Signal::HostCommand(OPEN_EDITOR_SENTINEL.to_owned()),
        "partial reply",
    );

    assert_eq!(outcome, ReplyOutcome::OpenEditor {
        current_text: "partial reply".into(),
    });
}

#[test]
fn builders_set_fields() {
    let reply = InlineReply::new("Reply:")
        .with_initial_text("seed")
        .with_help_message("Alt+Enter for newline")
        .with_edit_mode(ReplyEditMode::Vi);

    assert_eq!(reply.message, "Reply:");
    assert_eq!(reply.initial_text, "seed");
    assert_eq!(reply.help_message.as_deref(), Some("Alt+Enter for newline"));
    assert_eq!(reply.edit_mode, ReplyEditMode::Vi);
}
