pub fn hyperlink(uri: impl AsRef<str>, text: impl AsRef<str>) -> String {
    format!("\x1b]8;;{}\x07{}\x1b]8;;\x07", uri.as_ref(), text.as_ref())
}
