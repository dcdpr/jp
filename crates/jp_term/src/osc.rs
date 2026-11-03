use crate::sanitize::strip_controls;

/// Wrap `text` in an OSC 8 hyperlink to `uri`.
///
/// Every control character is removed from `uri` first, so it cannot end the
/// sequence early and write the rest of itself to the terminal as raw input.
/// `text` is written as given: it is displayed between the two sequences rather
/// than embedded in one, and may carry styling.
pub fn hyperlink(uri: impl AsRef<str>, text: impl AsRef<str>) -> String {
    format!(
        "\x1b]8;;{}\x07{}\x1b]8;;\x07",
        strip_controls(uri.as_ref(), &[]),
        text.as_ref()
    )
}

/// Write a terminal title using the OSC 2 escape sequence.
///
/// Terminals that don't support OSC 2 ignore the sequence.
/// The title appears in the terminal's tab or title bar.
/// Every control character is removed from `title` first, so it cannot end the
/// sequence early and write the rest of itself to the terminal as raw input.
///
/// Callers are responsible for checking whether they're connected to a terminal
/// before invoking this function.
/// Emitting OSC bytes into a non-TTY stderr (a captured pipe, a CI log, a
/// subprocess wrapper) pollutes the captured output without any visible effect.
pub fn set_title(title: impl AsRef<str>) {
    eprint!("{}", title_sequence(title.as_ref()));
}

/// The OSC 2 sequence that sets the window title to `title`, with every control
/// character removed from it.
fn title_sequence(title: &str) -> String {
    format!("\x1b]2;{}\x07", strip_controls(title, &[]))
}

#[cfg(test)]
#[path = "osc_tests.rs"]
mod tests;
