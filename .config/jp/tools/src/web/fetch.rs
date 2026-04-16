//! Dispatcher for the `web_fetch` tool.
//!
//! Picks a fetch strategy (HTML, markdown, or auto) based on the URL and the
//! user-configured `tool.options`, and delegates to the matching pipeline.

use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};
use serde_json::{Map, Value};
use url::Url;

use crate::util::ToolResult;

mod html;
mod markdown;
mod options;

use options::{Strategy, WebFetchOptions};

/// Content size limit (in bytes) above which we try LLM summarization.
pub(super) const SUMMARIZE_THRESHOLD: usize = 200_000;

pub(crate) async fn web_fetch(
    url: Url,
    list_sections: bool,
    sections: Option<Vec<String>>,
    options: &Map<String, Value>,
) -> ToolResult {
    let options = WebFetchOptions::parse(options);

    match options.pick_strategy(&url) {
        Strategy::Html => html::fetch(&url, list_sections, sections).await,
        Strategy::Markdown => markdown::fetch(&url, list_sections, sections).await,
        Strategy::Auto => {
            if let Some(result) =
                markdown::try_fetch(&url, list_sections, sections.as_deref()).await
            {
                return Ok(result.into());
            }
            html::fetch(&url, list_sections, sections).await
        }
    }
}

pub(super) fn http_client() -> reqwest::Client {
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

pub(super) fn is_binary(content_type: &str) -> bool {
    let ct = content_type.to_ascii_lowercase();
    ct.starts_with("image/")
        || ct.starts_with("audio/")
        || ct.starts_with("video/")
        || ct.starts_with("application/octet-stream")
        || ct.starts_with("application/pdf")
        || ct.starts_with("application/zip")
}

pub(super) fn truncate(content: &str, max: usize) -> String {
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

#[cfg(test)]
#[path = "fetch_tests.rs"]
mod tests;
