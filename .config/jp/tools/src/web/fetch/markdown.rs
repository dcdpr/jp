//! Markdown fetch pipeline.
//!
//! Tries to retrieve the `.md` variant of a URL (the Mintlify/GitBook/Fumadocs
//! convention of serving a markdown twin at `{path}.md`). Section listing and
//! extraction operate directly on the markdown AST (ATX headings).

use std::collections::HashMap;

use reqwest::header::CONTENT_TYPE;
use url::Url;

use super::{SUMMARIZE_THRESHOLD, http_client, truncate};
use crate::{
    Error,
    util::{ToolResult, error},
};

/// Strict fetch: errors propagate. Used by `Strategy::Markdown`.
pub(super) async fn fetch(
    url: &Url,
    list_sections: bool,
    sections: Option<Vec<String>>,
) -> ToolResult {
    let Some(md_url) = to_markdown_url(url) else {
        return error(format!(
            "URL {url} is not suitable for a markdown variant (path has no file segment or is \
             already `.md`)"
        ));
    };

    let body = fetch_markdown_body(&md_url).await?;
    Ok(process(&body, url, list_sections, sections.as_deref()).into())
}

/// Opportunistic fetch: swallows errors. Used by `Strategy::Auto` to probe the
/// `.md` variant before falling back to HTML.
pub(super) async fn try_fetch(
    url: &Url,
    list_sections: bool,
    sections: Option<&[String]>,
) -> Option<String> {
    let md_url = to_markdown_url(url)?;
    let body = fetch_markdown_body(&md_url).await.ok()?;
    Some(process(&body, url, list_sections, sections))
}

async fn fetch_markdown_body(url: &Url) -> Result<String, Error> {
    let response = http_client().get(url.clone()).send().await?;

    let status = response.status();
    if !status.is_success() {
        return Err(format!("`.md` variant returned HTTP {status}").into());
    }

    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();

    if !is_acceptable_markdown_content_type(&content_type) {
        return Err(
            format!("`.md` variant served with non-markdown Content-Type: {content_type}").into(),
        );
    }

    response.text().await.map_err(Into::into)
}

/// Accept text/* except `text/html` (the common soft-404 shape). Reject
/// anything that's clearly not text.
fn is_acceptable_markdown_content_type(ct: &str) -> bool {
    let ct = ct.trim().to_ascii_lowercase();

    // Reject HTML explicitly: servers that don't know about `.md` routes often
    // 200 with an HTML "page not found" shell.
    if ct.starts_with("text/html") || ct.starts_with("application/xhtml") {
        return false;
    }

    // Accept common markdown and text variants.
    if ct.starts_with("text/markdown")
        || ct.starts_with("text/x-markdown")
        || ct.starts_with("text/plain")
        || ct.starts_with("application/markdown")
    {
        return true;
    }

    // Missing content-type is common for static file hosts. Trust it.
    if ct.is_empty() {
        return true;
    }

    // Any other text/* is fine. Anything else is suspicious.
    ct.starts_with("text/")
}

/// Build the `.md` variant URL, preserving query and fragment.
///
/// Returns `None` if the URL is unsuitable (e.g. no path, already `.md`).
pub(super) fn to_markdown_url(url: &Url) -> Option<Url> {
    let path = url.path();

    if path.len() >= 3 && path.as_bytes()[path.len() - 3..].eq_ignore_ascii_case(b".md") {
        return None;
    }

    let path = path.trim_end_matches('/');
    if path.is_empty() {
        return None;
    }

    let path = path
        .strip_suffix(".html")
        .or_else(|| path.strip_suffix(".htm"))
        .unwrap_or(path);

    let mut out = url.clone();
    out.set_path(&format!("{path}.md"));
    Some(out)
}

fn process(
    body: &str,
    original_url: &Url,
    list_sections: bool,
    sections: Option<&[String]>,
) -> String {
    if list_sections {
        return format_section_listing(body);
    }

    if let Some(ids) = sections {
        let extracted = extract_sections(body, ids);
        return truncate(&extracted, SUMMARIZE_THRESHOLD);
    }

    // Respect URL fragments (`#some-section`) the same way the HTML pipeline
    // does: narrow the output to that section if the anchor resolves.
    let body = match original_url.fragment() {
        Some(frag) => {
            let ids = [frag.to_owned()];
            let extracted = extract_sections(body, &ids);
            if extracted.trim().is_empty() {
                body.to_owned()
            } else {
                extracted
            }
        }
        None => body.to_owned(),
    };

    truncate(&body, SUMMARIZE_THRESHOLD)
}

/// A parsed ATX heading.
#[derive(Debug, Clone)]
struct Heading {
    level: u8,
    slug: String,
    text: String,
    /// Zero-indexed line number of the heading in the source.
    line: usize,
}

/// Parse ATX-style (`#` prefixed) headings from markdown, skipping content
/// inside fenced code blocks. Setext (underlined) headings are not supported.
fn parse_headings(md: &str) -> Vec<Heading> {
    let mut out = Vec::new();
    let mut slugger = Slugger::default();
    let mut fence: Option<&str> = None;

    for (i, line) in md.lines().enumerate() {
        let trimmed = line.trim_start();

        if let Some(current) = fence {
            if trimmed.starts_with(current) {
                fence = None;
            }
            continue;
        }

        if trimmed.starts_with("```") {
            fence = Some("```");
            continue;
        }
        if trimmed.starts_with("~~~") {
            fence = Some("~~~");
            continue;
        }

        let Some(level) = atx_level(trimmed) else {
            continue;
        };

        let text = trimmed[level as usize..]
            .trim_start()
            .trim_end()
            .trim_end_matches('#')
            .trim()
            .to_owned();

        if text.is_empty() {
            continue;
        }

        let slug = slugger.slug(&text);
        out.push(Heading {
            level,
            slug,
            text,
            line: i,
        });
    }

    out
}

/// Return the ATX heading level of a line (1..=6), or `None` if it's not an
/// ATX heading. The character after the `#`s must be whitespace (per
/// CommonMark).
fn atx_level(line: &str) -> Option<u8> {
    let count = line.bytes().take_while(|b| *b == b'#').count();
    if !(1..=6).contains(&count) {
        return None;
    }

    let rest = &line[count..];
    if rest.is_empty() || rest.starts_with(|c: char| c.is_whitespace()) {
        u8::try_from(count).ok()
    } else {
        None
    }
}

/// Convert heading text into a GitHub-style slug:
/// - lowercase
/// - alphanumerics, `-`, `_` retained
/// - whitespace runs collapsed to a single `-`
/// - other punctuation stripped
fn slugify(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_dash = false;

    for ch in text.chars() {
        if ch.is_alphanumeric() {
            for lower in ch.to_lowercase() {
                out.push(lower);
            }
            last_dash = false;
        } else if ch == '-' || ch == '_' {
            out.push(ch);
            last_dash = ch == '-';
        } else if ch.is_whitespace() && !out.is_empty() && !last_dash {
            out.push('-');
            last_dash = true;
        }
    }

    let trimmed = out.trim_matches('-');
    trimmed.to_owned()
}

/// Slugger that de-duplicates by appending `-1`, `-2`, etc. Matches the
/// GitHub-flavored markdown convention.
#[derive(Default)]
struct Slugger {
    seen: HashMap<String, usize>,
}

impl Slugger {
    fn slug(&mut self, text: &str) -> String {
        let base = slugify(text);
        if base.is_empty() {
            return String::new();
        }

        let count = self.seen.entry(base.clone()).or_insert(0);
        let out = if *count == 0 {
            base.clone()
        } else {
            format!("{base}-{count}")
        };
        *count += 1;
        out
    }
}

/// Build an XML listing of all headings in the markdown document, mirroring
/// the HTML pipeline's format exactly.
fn format_section_listing(md: &str) -> String {
    let headings = parse_headings(md);
    if headings.is_empty() {
        return "No sections with anchors found on this page.".to_owned();
    }

    let lines: Vec<&str> = md.lines().collect();
    let mut out = String::from("<sections>\n");

    for (idx, h) in headings.iter().enumerate() {
        if h.slug.is_empty() {
            continue;
        }
        let preview = preview_after(&lines, &headings, idx);
        let body = if preview.is_empty() {
            h.text.clone()
        } else {
            format!("{} - {}", h.text, preview)
        };
        out.push_str(&format!(
            "  <s id=\"{}\" level=\"{}\">{}</s>\n",
            h.slug, h.level, body
        ));
    }

    out.push_str("</sections>");
    out
}

/// Maximum chars of preview text per section.
const PREVIEW_MAX: usize = 120;

/// Collect a short plain-text preview from content after the heading at
/// `idx`, stopping at the next heading of same-or-higher level.
fn preview_after(lines: &[&str], headings: &[Heading], idx: usize) -> String {
    let h = &headings[idx];
    let start = h.line + 1;
    let end = next_section_boundary(headings, idx, lines.len());

    let mut text = String::new();
    for raw in &lines[start..end] {
        let line = raw.trim();
        if line.is_empty() {
            if !text.is_empty() {
                break;
            }
            continue;
        }
        // Skip fence delimiters and horizontal rules.
        if line.starts_with("```") || line.starts_with("~~~") || line == "---" || line == "***" {
            continue;
        }
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(line);
        if text.len() >= PREVIEW_MAX {
            break;
        }
    }

    truncate_preview(&text, PREVIEW_MAX)
}

fn truncate_preview(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let end = s.floor_char_boundary(max);
    format!("{}...", &s[..end])
}

/// Line index of the next heading at `<= headings[idx].level`, or
/// `total_lines` if none.
fn next_section_boundary(headings: &[Heading], idx: usize, total_lines: usize) -> usize {
    let current_level = headings[idx].level;
    headings
        .iter()
        .skip(idx + 1)
        .find(|h| h.level <= current_level)
        .map_or(total_lines, |h| h.line)
}

/// Extract one or more sections by slug ID, concatenated into a single
/// markdown string. Missing IDs are silently skipped.
///
/// If no IDs match, returns an empty string (caller decides what to do).
fn extract_sections(md: &str, ids: &[String]) -> String {
    let headings = parse_headings(md);
    let lines: Vec<&str> = md.lines().collect();

    let mut chunks = Vec::new();
    for id in ids {
        let Some((idx, h)) = headings.iter().enumerate().find(|(_, h)| h.slug == *id) else {
            continue;
        };

        let end = next_section_boundary(&headings, idx, lines.len());
        let chunk = lines[h.line..end].join("\n");
        chunks.push(chunk.trim_end().to_owned());
    }

    chunks.join("\n\n")
}

#[cfg(test)]
#[path = "markdown_tests.rs"]
mod tests;
