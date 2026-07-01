//! HTML fetch pipeline.
//!
//! Fetches the URL as HTML, converts to markdown, and optionally extracts
//! anchor-scoped sections.
//! Pages that exceed `SUMMARIZE_THRESHOLD` after conversion are optionally run
//! through Haiku summarization if an API key is available.

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

/// Top-level wrapper IDs used by rustdoc-generated pages (docs.rs, local `cargo
/// doc` output).
/// These get matched as section anchors via the ancestor-id fallback in
/// `resolve_heading_id`, but they're page chrome, not content.
/// Filtering them out only at the listing layer keeps explicit
/// `sections=["main-content"]` requests working for non-rustdoc pages.
const RUSTDOC_SCAFFOLDING_IDS: &[&str] = &[
    "rustdoc_body_wrapper",
    "rustdoc-toc",
    "rustdoc-modnav",
    "main-content",
];

fn is_rustdoc_scaffolding_id(id: &str) -> bool {
    RUSTDOC_SCAFFOLDING_IDS.contains(&id)
}

/// True if `el` is a `<div class="docblock">` (rustdoc's documentation
/// container).
/// Multi-class attributes are handled.
fn is_docblock(el: &ElementRef<'_>) -> bool {
    el.value().name() == "div"
        && el
            .value()
            .attr("class")
            .is_some_and(|c| c.split_ascii_whitespace().any(|cl| cl == "docblock"))
}

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

#[cfg_attr(test, derive(Debug))]
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
    let section_html = extract_section_html_from_doc(&doc, anchor)?;

    let head_html = Selector::parse("head")
        .ok()
        .and_then(|s| doc.select(&s).next())
        .map(|el| el.html())
        .unwrap_or_default();

    Some(format!(
        "<html>{head_html}<body>{section_html}</body></html>"
    ))
}

/// Build the extraction for a rustdoc `<section>` item: the section itself plus
/// the immediately following sibling `<div class="docblock">`.
///
/// When the section is wrapped in `<summary>` (rustdoc's `<details>` toggle
/// layout), the docblock is a sibling of the summary, not of the section.
/// We do **not** return the whole `<details>` because impl headers nest every
/// method inside a `<div class="impl-items">` sibling — returning the details
/// would pull in every method's signature and docs.
fn extract_rustdoc_section(section: &ElementRef<'_>) -> String {
    let mut parts = vec![section.html()];

    let docblock_anchor = section
        .parent()
        .and_then(ElementRef::wrap)
        .filter(|el| el.value().name() == "summary")
        .unwrap_or(*section);

    for sib in docblock_anchor.next_siblings() {
        let Some(el) = ElementRef::wrap(sib) else {
            continue;
        };
        if is_docblock(&el) {
            parts.push(el.html());
        }
        // First element sibling decides: docblock or unrelated, stop either way.
        break;
    }

    parts.join("")
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

/// A section root is the element that anchors a stand-alone region of the
/// document.
/// Headings are the obvious case; AsciiDoctor-style horizontal definition lists
/// (`<dl><dt>term</dt><dd>...</dd></dl>`, used by git's manpages and many
/// AsciiDoc-generated sites) are the other common one.
enum SectionRoot<'a> {
    Heading(ElementRef<'a>),
    DefinitionTerm(ElementRef<'a>),
}

/// Walk up the ancestor chain looking for the closest section root.
/// A `<dt>` nested inside a heading section returns the `<dt>` (it's deeper, so
/// it's the more specific section).
fn find_section_root_ancestor<'a>(el: &ElementRef<'a>) -> Option<SectionRoot<'a>> {
    let mut node = el.parent()?;
    loop {
        if let Some(element) = ElementRef::wrap(node) {
            if element.value().name() == "dt" {
                return Some(SectionRoot::DefinitionTerm(element));
            }
            if is_heading(element.value().name()) {
                return Some(SectionRoot::Heading(element));
            }
        }
        node = node.parent()?;
    }
}

/// Extract a `<dt>` along with its associated `<dd>`s, wrapped in a fresh
/// `<dl>` so the result is valid standalone HTML.
///
/// `AsciiDoctor` sometimes emits multiple `<dd>`s per term (multi-paragraph
/// definitions), so we collect every `<dd>` until the next group starts.
/// HTML also allows the `<dt><dt><dd>` shape where several terms share one
/// definition; we accumulate consecutive sibling `<dt>`s up to the first `<dd>`
/// so fetching any term in the group returns the whole group.
/// Whitespace text nodes between siblings are preserved.
fn extract_definition_section(dt: &ElementRef<'_>) -> String {
    let mut parts = vec![dt.html()];
    let mut seen_dd = false;

    for sibling in dt.next_siblings() {
        if let Some(el) = ElementRef::wrap(sibling) {
            match el.value().name() {
                // Consecutive sibling `<dt>`s before any `<dd>` are part of
                // the same shared-definition group.
                "dt" if !seen_dd => parts.push(el.html()),
                "dd" => {
                    seen_dd = true;
                    parts.push(el.html());
                }
                // A `<dt>` after we've already collected `<dd>`s starts the
                // next group; anything else is unrelated.
                _ => break,
            }
        } else if let Some(text) = sibling.value().as_text() {
            parts.push(text.to_string());
        }
    }

    format!("<dl>{}</dl>", parts.join(""))
}

/// Resolve the anchor ID for a `<dt>` element.
/// Order differs from headings: `AsciiDoctor` wraps the canonical anchor in a
/// child `<a id="...">`, while the `<dt>`'s own `id` is auto-generated from the
/// term and tends to be ugly (e.g.
/// `Documentation/gitglossary.txt-aiddefcleanaclean`).
/// Prefer the child anchor when present.
fn resolve_dt_id(dt: &ElementRef<'_>) -> Option<String> {
    // Pattern 1: child <a id="...">.
    if let Ok(sel) = Selector::parse("a[id]")
        && let Some(child) = dt.select(&sel).next()
        && let Some(id) = child.value().attr("id")
        && !id.is_empty()
    {
        return Some(id.to_owned());
    }

    // Pattern 2: id on the <dt> itself.
    if let Some(id) = dt.value().attr("id")
        && !id.is_empty()
    {
        return Some(id.to_owned());
    }

    // Pattern 3: any descendant with id.
    if let Ok(sel) = Selector::parse("[id]")
        && let Some(child) = dt.select(&sel).next()
        && let Some(id) = child.value().attr("id")
        && !id.is_empty()
    {
        return Some(id.to_owned());
    }

    None
}

/// Collect a short plain-text preview from the `<dd>`s following a `<dt>`.
/// Mirrors the boundary rule in `extract_definition_section`: leading sibling
/// `<dt>`s in a shared-definition group are skipped (their preview comes from
/// the shared `<dd>`), `<dd>`s contribute, and anything else (or a `<dt>` after
/// we've started collecting) ends the preview.
fn extract_preview_after_dt(dt: &ElementRef<'_>) -> String {
    let mut text = String::new();
    let mut seen_dd = false;

    for sib in dt.next_siblings() {
        if let Some(el) = ElementRef::wrap(sib) {
            match el.value().name() {
                "dt" if !seen_dd => continue,
                "dd" => {
                    seen_dd = true;
                    let chunk: String = el.text().collect();
                    if !text.is_empty() && !chunk.is_empty() {
                        text.push(' ');
                    }
                    text.push_str(chunk.trim());
                }
                _ => break,
            }
        }
        if text.len() >= PREVIEW_MAX {
            break;
        }
    }

    truncate_str(&text, PREVIEW_MAX)
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

/// Build an XML listing of all anchored sections on the page (headings and
/// definition-list terms).
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

/// Discover all anchored sections in the document.
/// Walks `h1..h6` and `<dt>` elements in document order; each `<dt>` is
/// reported at one level below the most recent enclosing heading so
/// glossary-style pages produce a sensible outline.
fn list_section_headers(html: &str) -> Vec<SectionHeader> {
    let doc = Html::parse_document(html);
    let sel = Selector::parse("h1, h2, h3, h4, h5, h6, dt").unwrap();
    let mut seen_ids = std::collections::HashSet::new();
    let mut headers = Vec::new();
    let mut current_heading_level: u8 = 0;

    for el in doc.select(&sel) {
        if let Some(level) = heading_level(el.value().name()) {
            current_heading_level = level;

            let id = match resolve_heading_id(&el) {
                Some(id)
                    if !id.is_empty()
                        && !is_rustdoc_scaffolding_id(&id)
                        && seen_ids.insert(id.clone()) =>
                {
                    id
                }
                _ => continue,
            };

            headers.push(SectionHeader {
                id,
                level,
                text: clean_heading_text(&el),
                preview: extract_preview_after_heading(&el),
            });
        } else {
            // <dt> element. Skip if we can't resolve a usable anchor ID.
            let id = match resolve_dt_id(&el) {
                Some(id)
                    if !id.is_empty()
                        && !is_rustdoc_scaffolding_id(&id)
                        && seen_ids.insert(id.clone()) =>
                {
                    id
                }
                _ => continue,
            };

            // One level deeper than the parent heading, clamped to 6 (h6 is
            // the deepest level our XML output advertises).
            let level = current_heading_level.saturating_add(1).clamp(1, 6);

            headers.push(SectionHeader {
                id,
                level,
                text: clean_heading_text(&el),
                preview: extract_preview_after_dt(&el),
            });
        }
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

    // Rustdoc puts the signature heading inside `<section>` with no useful
    // following siblings; the prose lives in a sibling `<div class="docblock">`
    // of the section (or of its enclosing `<summary>` in toggle layout).
    // Without this fallback, every method on every docs.rs page reports an
    // empty preview.
    if text.is_empty()
        && let Some(parent) = heading.parent().and_then(ElementRef::wrap)
        && parent.value().name() == "section"
    {
        text = preview_from_section_docblock(parent);
    }

    truncate_str(&text, PREVIEW_MAX)
}

/// Collect preview text from a rustdoc section's neighboring docblock.
/// Mirrors `extract_rustdoc_section`'s sibling-resolution logic so listing
/// previews and extracted bodies agree on where the docs live.
fn preview_from_section_docblock(section: ElementRef<'_>) -> String {
    let anchor = section
        .parent()
        .and_then(ElementRef::wrap)
        .filter(|el| el.value().name() == "summary")
        .unwrap_or(section);

    for sib in anchor.next_siblings() {
        let Some(el) = ElementRef::wrap(sib) else {
            continue;
        };
        if is_docblock(&el) {
            let raw: String = el.text().collect();
            return raw.split_whitespace().collect::<Vec<_>>().join(" ");
        }
        break;
    }

    String::new()
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

    if target.value().name() == "dt" {
        return Some(extract_definition_section(&target));
    }

    if let Some(root) = find_section_root_ancestor(&target) {
        return Some(match root {
            SectionRoot::Heading(h) => extract_heading_section(&h),
            SectionRoot::DefinitionTerm(dt) => extract_definition_section(&dt),
        });
    }

    // Rustdoc wraps each item in `<section id="...">` (e.g. `method.X`,
    // `variant.Y`, `impl-Foo-for-Bar`). The signature heading is inside
    // the section; the documentation is in a sibling `<div class="docblock">`,
    // optionally further wrapped in a `<details>` toggle. The container
    // fallback below would only return the heading and miss the docs.
    if target.value().name() == "section" {
        return Some(extract_rustdoc_section(&target));
    }

    // Container element with an internal heading (e.g. `<section id="x"><h3>`).
    let heading_sel = Selector::parse("h1, h2, h3, h4, h5, h6").ok()?;
    if let Some(inner_heading) = target.select(&heading_sel).next() {
        return Some(extract_heading_section(&inner_heading));
    }

    Some(target.html())
}

#[cfg(test)]
#[path = "html_tests.rs"]
mod tests;
