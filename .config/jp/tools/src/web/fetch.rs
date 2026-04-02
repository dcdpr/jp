use htmd::HtmlToMarkdown;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue, USER_AGENT};
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

#[cfg(test)]
#[path = "fetch_tests.rs"]
mod tests;
