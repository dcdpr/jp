use super::*;

#[test]
fn emacs_keymap_binds_editor_escape() {
    let mut keybindings = default_emacs_keybindings();
    add_custom_bindings(&mut keybindings, true);

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
    add_custom_bindings(&mut keybindings, true);

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
    add_custom_bindings(&mut insert, true);

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
    add_custom_bindings(&mut normal, true);

    assert_eq!(
        normal.find_binding(KeyModifiers::CONTROL, KeyCode::Char('x')),
        Some(ReedlineEvent::ExecuteHostCommand(
            OPEN_EDITOR_SENTINEL.to_owned()
        ))
    );
}

#[test]
fn editor_escape_disabled_leaves_ctrl_x_unbound_but_keeps_newlines() {
    // `compose_in_editor = "never"`: the `Ctrl+X` escape must not be wired, but
    // the multi-line newline bindings still apply.
    let mut keybindings = default_emacs_keybindings();
    add_custom_bindings(&mut keybindings, false);

    assert_eq!(
        keybindings.find_binding(KeyModifiers::CONTROL, KeyCode::Char('x')),
        None
    );
    assert_eq!(
        keybindings.find_binding(KeyModifiers::ALT, KeyCode::Enter),
        Some(ReedlineEvent::Edit(vec![EditCommand::InsertNewline]))
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
fn the_buffer_shows_control_characters_as_question_marks() {
    // A tool result seeded into the buffer can carry escape sequences, and
    // reedline writes what the highlighter returns to the terminal as it is.
    // Line breaks and tabs are part of the text being edited.
    let shown = PlainBuffer.highlight("a\x1b[2Jb\tc\nd\r", 0);

    assert_eq!(shown.buffer, [
        (text_style(), "a".to_owned()),
        (marker_style(), "?".to_owned()),
        (text_style(), "[2Jb\tc\nd".to_owned()),
        (marker_style(), "?".to_owned()),
    ]);
}

#[test]
fn a_stand_in_takes_the_bytes_of_the_character_it_replaces() {
    // reedline cuts what it paints at the cursor's byte offset into the buffer,
    // so the two have to line up at every place the cursor can be. U+009B is
    // two bytes long.
    let line = "\u{e9}\u{9b}x\x7f\u{65e5}";
    let shown = PlainBuffer.highlight(line, 0);
    let expected = "\u{e9}??x?\u{65e5}";
    assert_eq!(shown.raw_string(), expected);

    let prompt = ReplyPrompt {
        message: String::new(),
        help: String::new(),
    };
    let cursors = line.char_indices().map(|(at, _)| at).chain([line.len()]);
    for at in cursors {
        let painted = shown.render_around_insertion_point(at, &prompt, false, None);

        assert_eq!(
            painted,
            (expected[..at].to_owned(), expected[at..].to_owned()),
            "cursor at byte {at}"
        );
    }
}

#[test]
fn builders_set_fields() {
    let reply = InlineReply::new("Reply:")
        .with_initial_text("seed")
        .with_help_message("Alt+Enter for newline")
        .with_edit_mode(ReplyEditMode::Vi)
        .with_editor_escape(false);

    assert_eq!(reply.message, "Reply:");
    assert_eq!(reply.initial_text, "seed");
    assert_eq!(reply.help_message.as_deref(), Some("Alt+Enter for newline"));
    assert_eq!(reply.edit_mode, ReplyEditMode::Vi);
    assert!(!reply.editor_escape);
}
