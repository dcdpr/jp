//! HTML fetch pipeline.
//!
//! Fetches the URL as HTML, converts to markdown, and optionally extracts
//! anchor-scoped sections. Pages that exceed `SUMMARIZE_THRESHOLD` after
//! conversion are optionally run through Haiku summarization if an API key
//! is available.

use htmd::HtmlToMarkdown;
use reqwest::header::CONTENT_TYPE;
use scraper::{ElementRef, Html, Selector};
use serde::Deserialize;
use url::Url;

use super::{SUMMARIZE_THRESHOLD, http_client, is_binary, truncate};
use crate::{
    Error,
    util::{ToolResult, error},
};

/// Max chars of preview text per section in the listing.
const PREVIEW_MAX: usize = 120;

const HAIKU_MODEL: &str = "claude-haiku-4-5";
const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";

pub(super) async fn fetch(
    url: &Url,
    list_sections: bool,
    sections: Option<Vec<String>>,
) -> ToolResult {
    let response = http_client().get(url.clone()).send().await?;

    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();

    if is_binary(&content_type) {
        return error(format!(
            "Cannot fetch binary content (Content-Type: {content_type})"
        ));
    }

    let body = response.text().await?;

    if !content_type.contains("html") {
        return Ok(truncate(&body, SUMMARIZE_THRESHOLD).into());
    }

    if list_sections {
        return Ok(format_section_listing(&body).into());
    }

    if let Some(ids) = sections {
        let html = extract_sections(&body, &ids);
        let md = html_to_markdown(&html)?;
        return Ok(truncate(&md, SUMMARIZE_THRESHOLD).into());
    }

    let body = match url.fragment() {
        Some(fragment) => extract_anchor_html(&body, fragment).unwrap_or(body),
        None => body,
    };

    let md = html_to_markdown(&body)?;

    if md.len() <= SUMMARIZE_THRESHOLD {
        return Ok(md.into());
    }

    // Try Haiku summarization for large pages.
    if let Some(summary) = try_summarize(url, &md).await {
        return Ok(summary.into());
    }

    Ok(truncate(&md, SUMMARIZE_THRESHOLD).into())
}

struct SectionHeader {
    id: String,
    level: u8,
    text: String,
    preview: String,
}

fn html_to_markdown(html: &str) -> Result<String, Error> {
    let converter = HtmlToMarkdown::builder()
        .skip_tags(vec!["script", "style", "noscript", "svg", "iframe"])
        .build();

    let md = converter.convert(html)?;
    Ok(collapse_blank_lines(&md))
}

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

async fn try_summarize(url: &Url, content: &str) -> Option<String> {
    let api_key = std::env::var("ANTHROPIC_API_KEY").ok()?;
    if api_key.is_empty() {
        return None;
    }

    match summarize(&api_key, url, content).await {
        Ok(summary) => Some(summary),
        Err(e) => {
            eprintln!("Haiku summarization failed, falling back to truncation: {e}");
            None
        }
    }
}

async fn summarize(api_key: &str, url: &Url, content: &str) -> Result<String, Error> {
    let prompt = format!(
        "Summarize the following web page content from <url>{url}</url>.\nPreserve key technical \
         details, code examples, API signatures, and important information. Be concise but \
         thorough.\n\n<content>\n{content}\n</content>"
    );

    let body = serde_json::json!({
        "model": HAIKU_MODEL,
        "max_tokens": 8_192,
        "messages": [{"role": "user", "content": prompt}]
    });

    let resp = http_client()
        .post(ANTHROPIC_API_URL)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&body)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("Anthropic API error ({status}): {text}").into());
    }

    let parsed: HaikuResponse = resp.json().await?;

    let summary_text = parsed
        .content
        .iter()
        .filter(|b| b.block_type == "text")
        .map(|b| b.text.as_str())
        .collect::<Vec<_>>()
        .join("");

    let original_size = content.len();
    Ok(format!(
        "[Content summarized from {original_size} bytes using {HAIKU_MODEL}]\n\n{summary_text}"
    ))
}

#[derive(Deserialize)]
struct HaikuResponse {
    content: Vec<ContentBlock>,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    #[serde(default)]
    text: String,
}

/// Given raw HTML and a URL fragment (anchor), extracts the section targeted by
/// that anchor and returns a new HTML document containing the original `<head>`
/// but with only the extracted section in the `<body>`.
///
/// Returns `None` if the anchor element isn't found, letting the caller fall
/// back to the full page.
fn extract_anchor_html(html: &str, anchor: &str) -> Option<String> {
    let doc = Html::parse_document(html);

    let selector = Selector::parse(&format!("[id=\"{}\"]", escape_css_value(anchor))).ok()?;
    let target = doc.select(&selector).next()?;

    let section_html = if is_heading(target.value().name()) {
        extract_heading_section(&target)
    } else if let Some(heading) = find_heading_ancestor(&target) {
        extract_heading_section(&heading)
    } else {
        target.html()
    };

    let head_html = Selector::parse("head")
        .ok()
        .and_then(|s| doc.select(&s).next())
        .map(|el| el.html())
        .unwrap_or_default();

    Some(format!(
        "<html>{head_html}<body>{section_html}</body></html>"
    ))
}

/// Extracts a heading element and all following siblings up to (but not
/// including) the next heading of the same or higher level.
fn extract_heading_section(heading: &ElementRef<'_>) -> String {
    let level = heading_level(heading.value().name()).unwrap_or(0);
    let mut parts = vec![heading.html()];

    for sibling in heading.next_siblings() {
        if let Some(el) = ElementRef::wrap(sibling) {
            if let Some(sib_level) = heading_level(el.value().name())
                && sib_level <= level
            {
                break;
            }
            parts.push(el.html());
        } else if let Some(text) = sibling.value().as_text() {
            parts.push(text.to_string());
        }
    }

    parts.join("")
}

fn is_heading(tag: &str) -> bool {
    heading_level(tag).is_some()
}

fn heading_level(tag: &str) -> Option<u8> {
    match tag {
        "h1" => Some(1),
        "h2" => Some(2),
        "h3" => Some(3),
        "h4" => Some(4),
        "h5" => Some(5),
        "h6" => Some(6),
        _ => None,
    }
}

/// Walk up the ancestor chain looking for a heading element.
fn find_heading_ancestor<'a>(el: &ElementRef<'a>) -> Option<ElementRef<'a>> {
    let mut node = el.parent()?;
    loop {
        if let Some(element) = ElementRef::wrap(node)
            && is_heading(element.value().name())
        {
            return Some(element);
        }
        node = node.parent()?;
    }
}

/// Escape characters that have special meaning in CSS attribute value
/// selectors.
fn escape_css_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' | '\\' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out
}

/// Build an XML listing of all headed sections on the page.
fn format_section_listing(html: &str) -> String {
    let headers = list_section_headers(html);
    if headers.is_empty() {
        return "No sections with anchors found on this page.".to_owned();
    }

    let mut out = String::from("<sections>\n");
    for h in &headers {
        let preview = if h.preview.is_empty() {
            h.text.clone()
        } else {
            format!("{} - {}", h.text, h.preview)
        };
        out.push_str(&format!(
            "  <s id=\"{}\" level=\"{}\">{}</s>\n",
            h.id, h.level, preview
        ));
    }
    out.push_str("</sections>");
    out
}

/// Discover all heading elements that have an associated anchor ID.
fn list_section_headers(html: &str) -> Vec<SectionHeader> {
    let doc = Html::parse_document(html);
    let heading_sel = Selector::parse("h1, h2, h3, h4, h5, h6").unwrap();
    let mut seen_ids = std::collections::HashSet::new();
    let mut headers = Vec::new();

    for heading in doc.select(&heading_sel) {
        let Some(level) = heading_level(heading.value().name()) else {
            continue;
        };

        let id = resolve_heading_id(&heading);
        let id = match id {
            Some(id) if !id.is_empty() && seen_ids.insert(id.clone()) => id,
            _ => continue,
        };

        let text = clean_heading_text(&heading);
        let preview = extract_preview_after_heading(&heading);

        headers.push(SectionHeader {
            id,
            level,
            text,
            preview,
        });
    }

    headers
}

/// Resolve the anchor ID for a heading, trying multiple patterns:
///
/// 1. `id` attribute on the heading itself
/// 2. `id` attribute on a parent element (e.g. `<section id="..."><h3>`)
/// 3. `href="#id"` on a child anchor (permalink pattern)
/// 4. `id` attribute on a child element (e.g. `<h3><div id="...">`)
/// 5. `id` or `name` on the immediately preceding sibling element
fn resolve_heading_id(heading: &ElementRef<'_>) -> Option<String> {
    // Pattern 1: id on heading.
    if let Some(id) = heading.value().attr("id") {
        return Some(id.to_owned());
    }

    // Pattern 2: id on ancestor (walk up through non-heading parents).
    {
        let mut node = heading.parent();
        while let Some(n) = node {
            if let Some(el) = ElementRef::wrap(n) {
                if is_heading(el.value().name()) {
                    break;
                }
                if let Some(id) = el.value().attr("id") {
                    return Some(id.to_owned());
                }
            }
            node = n.parent();
        }
    }

    // Pattern 3: child anchor with href="#..."
    if let Ok(sel) = Selector::parse("a[href^='#']")
        && let Some(anchor) = heading.select(&sel).next()
        && let Some(href) = anchor.value().attr("href")
    {
        let fragment = href.trim_start_matches('#');
        if !fragment.is_empty() {
            return Some(fragment.to_owned());
        }
    }

    // Pattern 4: child element with id.
    if let Ok(sel) = Selector::parse("[id]")
        && let Some(child) = heading.select(&sel).next()
        && let Some(id) = child.value().attr("id")
    {
        return Some(id.to_owned());
    }

    // Pattern 5: preceding sibling anchor with id or name.
    for sib in heading.prev_siblings() {
        if let Some(el) = ElementRef::wrap(sib) {
            if el.value().name() == "a"
                && let Some(id) = el.value().attr("id").or_else(|| el.value().attr("name"))
            {
                return Some(id.to_owned());
            }

            // Only check the immediately preceding element.
            break;
        }

        // Skip text nodes (whitespace between elements).
        if let Some(text) = sib.value().as_text()
            && text.trim().is_empty()
        {
            continue;
        }

        break;
    }

    None
}

/// Get plain heading text, stripping permalink symbols like "¶".
fn clean_heading_text(heading: &ElementRef<'_>) -> String {
    let raw = heading.text().collect::<String>();
    raw.trim()
        .trim_end_matches('¶')
        .trim_end_matches('#')
        .trim()
        .to_owned()
}

/// Collect a short plain-text preview from content after the heading.
fn extract_preview_after_heading(heading: &ElementRef<'_>) -> String {
    let mut text = String::new();
    let level = heading_level(heading.value().name()).unwrap_or(0);

    for sib in heading.next_siblings() {
        if let Some(el) = ElementRef::wrap(sib) {
            // Stop at next heading of same or higher level.
            if let Some(l) = heading_level(el.value().name())
                && l <= level
            {
                break;
            }

            let chunk: String = el.text().collect();
            if !text.is_empty() && !chunk.is_empty() {
                text.push(' ');
            }

            text.push_str(chunk.trim());
        } else if let Some(t) = sib.value().as_text() {
            let t = t.trim();
            if !t.is_empty() {
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(t);
            }
        }
        if text.len() >= PREVIEW_MAX {
            break;
        }
    }

    truncate_str(&text, PREVIEW_MAX)
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let end = s.floor_char_boundary(max);

    format!("{}...", &s[..end])
}

/// Extract multiple sections by anchor ID and combine into one HTML document.
fn extract_sections(html: &str, ids: &[String]) -> String {
    let doc = Html::parse_document(html);

    let head_html = Selector::parse("head")
        .ok()
        .and_then(|s| doc.select(&s).next())
        .map(|el| el.html())
        .unwrap_or_default();

    let mut body_parts = Vec::new();
    for id in ids {
        if let Some(section) = extract_section_html_from_doc(&doc, id) {
            body_parts.push(section);
        }
    }

    if body_parts.is_empty() {
        return html.to_owned();
    }

    format!(
        "<html>{head_html}<body>{}</body></html>",
        body_parts.join("\n")
    )
}

/// Extract a single section from an already-parsed document.
fn extract_section_html_from_doc(doc: &Html, anchor: &str) -> Option<String> {
    let selector = Selector::parse(&format!("[id=\"{}\"]", escape_css_value(anchor))).ok()?;
    let target = doc.select(&selector).next()?;

    if is_heading(target.value().name()) {
        return Some(extract_heading_section(&target));
    }

    if let Some(heading) = find_heading_ancestor(&target) {
        return Some(extract_heading_section(&heading));
    }

    // Check if the element contains a heading (e.g. <section id="..."><h3>...)
    let heading_sel = Selector::parse("h1, h2, h3, h4, h5, h6").ok()?;
    if let Some(inner_heading) = target.select(&heading_sel).next() {
        return Some(extract_heading_section(&inner_heading));
    }

    Some(target.html())
}

#[cfg(test)]
#[path = "html_tests.rs"]
mod tests;
