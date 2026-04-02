use std::{collections::BTreeSet, error::Error, marker::PhantomData};

use async_trait::async_trait;
use camino::Utf8Path;
use htmd::HtmlToMarkdown;
use jp_attachment::{
    Attachment, BoxedHandler, HANDLERS, Handler, distributed_slice, linkme, typetag,
};
use jp_mcp::Client;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderValue, USER_AGENT};
use serde::{Deserialize, Serialize};
use tracing::{debug, error};
use url::Url;

#[distributed_slice(HANDLERS)]
#[linkme(crate = linkme)]
static HTTP_HANDLER: fn() -> BoxedHandler = http_handler;

#[distributed_slice(HANDLERS)]
#[linkme(crate = linkme)]
static HTTPS_HANDLER: fn() -> BoxedHandler = https_handler;

fn http_handler() -> BoxedHandler {
    (Box::new(HttpContent::<Http>::default()) as Box<dyn Handler>).into()
}

fn https_handler() -> BoxedHandler {
    (Box::new(HttpContent::<Https>::default()) as Box<dyn Handler>).into()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct HttpContent<T> {
    urls: BTreeSet<Url>,
    _phantom: PhantomData<T>,
}

/// Marker type for HTTP URLs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
struct Http;

/// Marker type for HTTPS URLs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
struct Https;

#[typetag::serde(name = "http_content")]
#[async_trait]
impl Handler for HttpContent<Http> {
    fn scheme(&self) -> &'static str {
        "http"
    }

    async fn add(
        &mut self,
        uri: &Url,
        _cwd: &Utf8Path,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.urls.insert(uri.clone());
        Ok(())
    }

    async fn remove(&mut self, uri: &Url) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.urls.remove(uri);
        Ok(())
    }

    async fn list(&self) -> Result<Vec<Url>, Box<dyn Error + Send + Sync>> {
        Ok(self.urls.iter().cloned().collect())
    }

    async fn get(
        &self,
        _: &Utf8Path,
        _: Client,
    ) -> Result<Vec<Attachment>, Box<dyn Error + Send + Sync>> {
        debug!(id = "http", "Getting http attachment contents.");
        fetch_all(&self.urls).await
    }
}

#[typetag::serde(name = "https_content")]
#[async_trait]
impl Handler for HttpContent<Https> {
    fn scheme(&self) -> &'static str {
        "https"
    }

    async fn add(
        &mut self,
        uri: &Url,
        _cwd: &Utf8Path,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.urls.insert(uri.clone());

        Ok(())
    }

    async fn remove(&mut self, uri: &Url) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.urls.remove(uri);

        Ok(())
    }

    async fn list(&self) -> Result<Vec<Url>, Box<dyn Error + Send + Sync>> {
        Ok(self.urls.iter().cloned().collect())
    }

    async fn get(
        &self,
        _: &Utf8Path,
        _: Client,
    ) -> Result<Vec<Attachment>, Box<dyn Error + Send + Sync>> {
        debug!(id = "https", "Getting https attachment contents.");
        fetch_all(&self.urls).await
    }
}

async fn fetch_all(urls: &BTreeSet<Url>) -> Result<Vec<Attachment>, Box<dyn Error + Send + Sync>> {
    let mut attachments = vec![];
    for url in urls {
        let response = match http_client().get(url.clone()).send().await {
            Ok(response) => response,
            Err(error) => {
                error!(uri = %url, %error, "Failed to fetch HTTP content.");
                continue;
            }
        };

        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();

        if is_binary(&content_type) {
            error!(uri = %url, content_type, "Skipping binary HTTP content.");
            continue;
        }

        let body = response.text().await?;

        let content = if content_type.contains("html") {
            html_to_markdown(&body)?
        } else {
            body
        };

        attachments.push(Attachment::text(url.to_string(), content));
    }

    Ok(attachments)
}

fn html_to_markdown(html: &str) -> Result<String, Box<dyn Error + Send + Sync>> {
    let converter = HtmlToMarkdown::builder()
        .skip_tags(vec!["script", "style", "noscript", "svg", "iframe"])
        .build();

    let md = converter.convert(html)?;
    Ok(collapse_blank_lines(&md))
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

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
