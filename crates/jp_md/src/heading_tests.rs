use super::leading_heading;

#[test]
fn atx_heading() {
    assert_eq!(
        leading_heading("# Fix the parser"),
        Some("Fix the parser".to_owned())
    );
}

#[test]
fn atx_heading_levels() {
    assert_eq!(leading_heading("### Deep"), Some("Deep".to_owned()));
    assert_eq!(
        leading_heading("###### Deepest"),
        Some("Deepest".to_owned())
    );
}

#[test]
fn setext_heading() {
    assert_eq!(
        leading_heading("My Title\n========"),
        Some("My Title".to_owned())
    );
    assert_eq!(
        leading_heading("My Title\n--------"),
        Some("My Title".to_owned())
    );
}

#[test]
fn leading_blank_lines_are_allowed() {
    assert_eq!(leading_heading("\n\n# Heading"), Some("Heading".to_owned()));
}

#[test]
fn inline_code_keeps_backticks() {
    assert_eq!(
        leading_heading("# Fix `parse` bug"),
        Some("Fix `parse` bug".to_owned())
    );
    assert_eq!(
        leading_heading("# Use ``a `b` c`` here"),
        Some("Use ``a `b` c`` here".to_owned())
    );
}

#[test]
fn heading_after_paragraph_is_rejected() {
    assert_eq!(leading_heading("intro\n\n# Heading"), None);
}

#[test]
fn non_heading_start_is_rejected() {
    assert_eq!(leading_heading("just a sentence"), None);
    assert_eq!(leading_heading("- list item"), None);
    assert_eq!(leading_heading("```\ncode\n```"), None);
}

#[test]
fn empty_heading_is_rejected() {
    assert_eq!(leading_heading("#"), None);
    assert_eq!(leading_heading("# "), None);
}

#[test]
fn empty_input_is_rejected() {
    assert_eq!(leading_heading(""), None);
}
