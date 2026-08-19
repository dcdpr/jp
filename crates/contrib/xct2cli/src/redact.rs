//! Removes the recorded process's environment from exported XML.
//!
//! Instruments stores every environment variable the profiled process held, and
//! `xctrace export --toc` prints them back.
//! On a developer machine that is a full set of API keys and tokens.
//! Everything this crate receives from `xctrace` passes through
//! [`strip_environment`] before any caller sees it, so there is no path by
//! which the library hands those out.

/// Opening tag name, without its terminator.
const OPEN: &[u8] = b"<environment";
/// Closing tag, matched in full.
const CLOSE: &[u8] = b"</environment>";
/// What a stripped block becomes.
/// Keeps the document parseable and makes the removal visible to anyone reading
/// the output.
const REPLACEMENT: &[u8] = b"<environment redacted=\"true\"/>";

/// Replace every `<environment>...</environment>` block in `xml` with an empty,
/// marked element.
///
/// Self-closing `<environment/>` elements are left as they are; they hold
/// nothing.
/// Elements whose name merely starts with `environment`, such as
/// `<environment-info>`, are not touched.
///
/// Input that opens a block and never closes it is truncated at that point
/// rather than passed through, so malformed XML cannot become a disclosure.
#[must_use]
pub fn strip_environment(xml: Vec<u8>) -> Vec<u8> {
    let mut out = Vec::with_capacity(xml.len());
    let mut rest = xml.as_slice();

    while let Some(start) = find_element(rest) {
        let name_end = start + OPEN.len();
        let Some(tag_end) = find(&rest[name_end..], b">").map(|i| name_end + i) else {
            return truncate(out, &rest[..start]);
        };

        if rest[tag_end - 1] == b'/' {
            out.extend_from_slice(&rest[..=tag_end]);
            rest = &rest[tag_end + 1..];
            continue;
        }

        let Some(close) = find(&rest[tag_end..], CLOSE).map(|i| tag_end + i) else {
            return truncate(out, &rest[..start]);
        };

        out.extend_from_slice(&rest[..start]);
        out.extend_from_slice(REPLACEMENT);
        rest = &rest[close + CLOSE.len()..];
    }

    out.extend_from_slice(rest);
    out
}

/// Emit everything before an unterminated block, then stop.
fn truncate(mut out: Vec<u8>, head: &[u8]) -> Vec<u8> {
    out.extend_from_slice(head);
    out.extend_from_slice(REPLACEMENT);
    out
}

/// Offset of the next `<environment` that is a whole element name rather than
/// the prefix of a longer one.
fn find_element(haystack: &[u8]) -> Option<usize> {
    let mut from = 0;
    while let Some(offset) = find(&haystack[from..], OPEN) {
        let at = from + offset;
        match haystack.get(at + OPEN.len()) {
            Some(b'>' | b'/') => return Some(at),
            Some(c) if c.is_ascii_whitespace() => return Some(at),
            _ => from = at + OPEN.len(),
        }
    }
    None
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
#[path = "redact_tests.rs"]
mod tests;
