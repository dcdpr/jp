use htmd::HtmlToMarkdown;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue, USER_AGENT};
use scraper::{ElementRef, Html, Selector};
use serde::Deserialize;
use url::Url;

use crate::{
    Error,
    util::{ToolResult, error},
};

/// Content size limit (in bytes) above which we attempt LLM summarization.
const SUMMARIZE_THRESHOLD: usize = 200_000;

const HAIKU_MODEL: &str = "claude-haiku-4-5";
const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";

pub(crate) async fn web_fetch(url: Url) -> ToolResult {
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

    let body = match url.fragment() {
        Some(fragment) => extract_anchor_html(&body, fragment).unwrap_or(body),
        None => body,
    };

    if !content_type.contains("html") {
        return Ok(truncate(&body, SUMMARIZE_THRESHOLD).into());
    }

    let md = html_to_markdown(&body)?;

    if md.len() <= SUMMARIZE_THRESHOLD {
        return Ok(md.into());
    }

    // Try Haiku summarization for large pages
    if let Some(summary) = try_summarize(&url, &md).await {
        return Ok(summary.into());
    }

    Ok(truncate(&md, SUMMARIZE_THRESHOLD).into())
}

fn html_to_markdown(html: &str) -> Result<String, Error> {
    let converter = HtmlToMarkdown::builder()
        .skip_tags(vec!["script", "style", "noscript", "svg", "iframe"])
        .build();

    let md = converter.convert(html)?;
    Ok(collapse_blank_lines(&md))
}

fn is_binary(content_type: &str) -> bool {
    let ct = content_type.to_ascii_lowercase();
    ct.starts_with("image/")
        || ct.starts_with("audio/")
        || ct.starts_with("video/")
        || ct.starts_with("application/octet-stream")
        || ct.starts_with("application/pdf")
        || ct.starts_with("application/zip")
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

fn truncate(content: &str, max: usize) -> String {
    if content.len() <= max {
        return content.to_owned();
    }

    let end = content.floor_char_boundary(max);
    format!(
        "{}\n\n[Content truncated: showing {end} of {} bytes]",
        &content[..end],
        content.len()
    )
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

fn http_client() -> reqwest::Client {
    let mut headers = HeaderMap::new();
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like \
             Gecko) Chrome/137.0.0.0 Safari/537.36",
        ),
    );

    reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .expect("failed to build HTTP client")
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
pub fn extract_anchor_html(html: &str, anchor: &str) -> Option<String> {
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

#[cfg(test)]
#[path = "fetch_tests.rs"]
mod tests;
