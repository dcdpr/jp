use htmd::HtmlToMarkdown;

use crate::error::Error;

/// Convert an HTML fragment to Markdown.
///
/// `script`, `style`, `noscript`, `svg`, and `iframe` are dropped (they're
/// page chrome, never useful content for an LLM).
pub(crate) fn html_to_markdown(html: &str) -> Result<String, Error> {
    let converter = HtmlToMarkdown::builder()
        .skip_tags(vec!["script", "style", "noscript", "svg", "iframe"])
        .build();

    converter
        .convert(html)
        .map(|md| collapse_blank_lines(&md))
        .map_err(|e| Error::HtmlToMarkdown(e.to_string()))
}

/// Cap runs of blank lines at two newlines, so the LLM sees `paragraph\n\nnext`
/// rather than the long runs `htmd` sometimes produces from rustdoc's
/// indentation-heavy HTML. Trailing whitespace is dropped.
fn collapse_blank_lines(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut consecutive_newlines = 0u8;

    for ch in s.chars() {
        if ch == '\n' {
            consecutive_newlines = consecutive_newlines.saturating_add(1);
            if consecutive_newlines <= 2 {
                out.push(ch);
            }
        } else {
            consecutive_newlines = 0;
            out.push(ch);
        }
    }

    out.truncate(out.trim_end().len());
    out
}

#[cfg(test)]
#[path = "util_tests.rs"]
mod tests;
