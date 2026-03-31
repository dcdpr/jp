pub fn hyperlink(uri: impl AsRef<str>, text: impl AsRef<str>) -> String {
    format!("\x1b]8;;{}\x07{}\x1b]8;;\x07", uri.as_ref(), text.as_ref())
}

/// Write a terminal title using the OSC 2 escape sequence.
///
/// Terminals that don't support OSC 2 ignore the sequence.
/// The title appears in the terminal's tab or title bar.
pub fn set_title(title: impl AsRef<str>) {
    // OSC 2 ; <title> ST
    eprint!("\x1b]2;{}\x07", title.as_ref());
}
