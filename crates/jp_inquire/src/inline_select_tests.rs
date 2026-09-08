use super::*;

#[test]
fn test_split_message_single_line() {
    let (body, prompt) = split_message("Continue?");
    assert_eq!(body, None);
    assert_eq!(prompt, "Continue?");
}

#[test]
fn test_split_message_multiline_keeps_last_line_as_prompt() {
    let (body, prompt) = split_message("diff line 1\ndiff line 2\nApply?");
    assert_eq!(body, Some("diff line 1\ndiff line 2"));
    assert_eq!(prompt, "Apply?");
}

#[test]
fn test_split_message_preserves_blank_separator() {
    // A trailing blank line between body and prompt is preserved in the
    // body so the rendered output keeps the visual separation.
    let (body, prompt) = split_message("patch contents\n\nApply patch?");
    assert_eq!(body, Some("patch contents\n"));
    assert_eq!(prompt, "Apply patch?");
}

#[test]
fn test_split_message_trailing_newline_yields_empty_prompt() {
    // Edge case: callers should not produce trailing newlines, but if
    // they do, the prompt is empty and the body holds everything else.
    let (body, prompt) = split_message("trailing newline\n");
    assert_eq!(body, Some("trailing newline"));
    assert_eq!(prompt, "");
}

#[test]
fn test_inline_option_new() {
    let opt = InlineOption::new('y', "yes - proceed");
    assert_eq!(opt.key, 'y');
    assert_eq!(opt.description, "yes - proceed");
}

/// The help message is a separate channel from the '?' listing: it reaches a
/// user who answers straight away, which is exactly the user a warning about an
/// irreversible action needs to reach.
#[test]
fn help_message_is_unset_until_asked_for_and_kept_out_of_the_option_listing() {
    let options = vec![InlineOption::new('y', "yes, remove it")];

    let plain = InlineSelect::new("Remove this?", options.clone());
    assert_eq!(plain.help_message, None);

    let warned = InlineSelect::new("Remove this?", options).with_help_message("cannot be undone");
    assert_eq!(warned.help_message.as_deref(), Some("cannot be undone"));

    // '?' still lists only the options, so the two do not duplicate each other.
    let help = warned.build_help_text().unwrap();
    assert_eq!(help, "y - yes, remove it\n? - print help");
}

#[test]
fn test_inline_select_build_help_text() {
    let options = vec![
        InlineOption::new('y', "stage this hunk"),
        InlineOption::new('n', "do not stage this hunk"),
        InlineOption::new('q', "quit"),
    ];
    let select = InlineSelect::new("Stage this hunk", options);
    let help = select.build_help_text().unwrap();

    assert!(help.contains("y - stage this hunk"));
    assert!(help.contains("n - do not stage this hunk"));
    assert!(help.contains("q - quit"));
    assert!(help.contains("? - print help"));
}
