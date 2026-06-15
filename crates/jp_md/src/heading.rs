//! Heading extraction from markdown text.

use comrak::{Arena, Options, nodes::NodeValue, parse_document};

/// Return the text of the leading heading in `text`, if the document starts
/// with one.
///
/// Returns `Some` only when the very first block of `text` is a markdown
/// heading, either ATX (`# Title`) or setext (a line underlined with `===` or
/// `---`).
/// The heading's text is returned as parsed, without trimming or truncation.
/// Any other leading block, a heading that is not first, or an empty heading
/// yields `None`.
#[must_use]
pub fn leading_heading(text: &str) -> Option<String> {
    let arena = Arena::new();
    let root = parse_document(&arena, text, &Options::default());

    let heading = root.first_child()?;
    if !matches!(heading.data().value, NodeValue::Heading(_)) {
        return None;
    }

    let mut title = String::new();
    for node in heading.descendants() {
        match node.data().value {
            NodeValue::Text(ref literal) => title.push_str(literal),
            NodeValue::Code(ref code) => {
                let fence = "`".repeat(code.num_backticks);
                title.push_str(&fence);
                title.push_str(&code.literal);
                title.push_str(&fence);
            }
            _ => {}
        }
    }

    (!title.is_empty()).then_some(title)
}

#[cfg(test)]
#[path = "heading_tests.rs"]
mod tests;
