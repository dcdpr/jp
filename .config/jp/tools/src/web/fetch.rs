//! Dispatcher for the `web_fetch` tool.
//!
//! Picks a fetch strategy (HTML, markdown, or auto) based on the URL and the
//! user-configured `tool.options`, and delegates to the matching pipeline.

use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};
use serde_json::{Map, Value};
use url::Url;

use crate::util::{ToolResult, error};

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
    // GitHub issue and PR pages render comments client-side, so the HTML
    // pipeline returns near-empty results for one of the most common URL
    // shapes a user will paste. Redirect to the dedicated tools rather
    // than silently failing.
    if let Some(redirect) = github_issue_or_pr_redirect(&url) {
        return error(redirect);
    }

    let options = match WebFetchOptions::parse(options) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("Error parsing options: {error}");
            WebFetchOptions::default()
        }
    };

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

/// If the URL points at a GitHub issue or PR, build a redirect message
/// suggesting the dedicated tool.
/// Returns `None` for any other URL.
///
/// We intentionally don't try to be smart about every kind of github.com URL —
/// blobs, releases, the repo root, etc. continue through the HTML pipeline
/// because for those the rendered HTML is enough.
fn github_issue_or_pr_redirect(url: &Url) -> Option<String> {
    if !matches!(url.host_str(), Some(host) if host.eq_ignore_ascii_case("github.com")) {
        return None;
    }

    let segments: Vec<&str> = url.path_segments()?.filter(|s| !s.is_empty()).collect();
    if segments.len() < 4 {
        return None;
    }

    let tool = match segments.as_slice() {
        [_, _, "issues", _, ..] => "github_issues",
        // The files-changed and commits tabs map to dedicated tools. Other PR
        // subpaths (checks, conflicts) fall through to `github_pulls`, where the
        // metadata+conversation answer is the closest fit.
        [_, _, "pull" | "pulls", _, "files", ..] => "github_pr_diff",
        [_, _, "pull" | "pulls", _, "commits", ..] => "github_pr_commits",
        [_, _, "pull" | "pulls", _, ..] => "github_pulls",
        _ => return None,
    };
    let owner = segments[0];
    let repo = segments[1];
    let number: u64 = segments[3].parse().ok()?;

    Some(format!(
        "GitHub issue and PR pages render comments client-side, so `web_fetch` can't return them. \
         Use the `{tool}` tool instead, for example:\n{{\"repository\": \"{owner}/{repo}\", \
         \"number\": {number}}}"
    ))
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
