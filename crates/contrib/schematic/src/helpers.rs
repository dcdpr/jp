/// Strip a leading BOM from the string.
#[must_use]
pub fn strip_bom(content: &str) -> &str {
    content.trim_start_matches("\u{feff}")
}

/// Extract a file name from the provided file path.
#[must_use]
fn extract_file_name(value: &str) -> &str {
    // Remove any query string
    let value = if let Some(index) = value.rfind('?') {
        &value[0..index]
    } else {
        value
    };

    // And only check the last segment
    if let Some(index) = value.rfind('/') {
        &value[index + 1..]
    } else {
        value
    }
}

/// Extract a file extension (without period) from the provided file path.
#[must_use]
pub fn extract_file_ext(value: &str) -> Option<&str> {
    let name = extract_file_name(value);

    name.rfind('.').map(|index| &name[index + 1..])
}
