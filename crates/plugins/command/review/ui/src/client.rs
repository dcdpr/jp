//! Requests to the JSON API of the `jp review serve` that served the page.
//!
//! The URL builders, [`Call`], and [`read`] are pure; [`get`], [`post`], and
//! [`send`] send the request with the browser's `fetch`.
//! A request goes to the page's own origin, which is what the server's
//! same-origin check on writes expects.

use std::ops::Range;

use jp_review_api::{Draft, ErrorBody};
use js_sys::Error;
use serde::{Serialize, de::DeserializeOwned};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Request, RequestInit, Response};

use crate::route::{Span, encode};

/// The HTTP statuses of an answer that did what was asked.
const SUCCESS: Range<u16> = 200..300;

/// `GET /ui/build`: the build a bundle directory holds, as a JSON string, when
/// `jp review serve --ui-dir` serves the UI.
pub const BUILD_URL: &str = "/ui/build";

/// `GET /api/reviews`.
#[must_use]
pub fn list_url() -> String {
    "/api/reviews".to_owned()
}

/// `GET /api/reviews/{id}`, leaving the review out of the answer unless it
/// changed since `revision`, with its comments placed in `version`, or the
/// latest.
#[must_use]
pub fn detail_url(id: &str, revision: Option<u64>, version: Option<u32>) -> String {
    let params: Vec<String> = [
        revision.map(|revision| format!("revision={revision}")),
        version.map(|version| format!("version={version}")),
    ]
    .into_iter()
    .flatten()
    .collect();

    let url = format!("/api/reviews/{}", encode(id));
    if params.is_empty() {
        return url;
    }

    format!("{url}?{}", params.join("&"))
}

/// `GET /api/reviews/{id}/versions/{version}/file`, rendered.
#[must_use]
pub fn file_url(id: &str, version: u32, path: &str) -> String {
    format!(
        "/api/reviews/{}/versions/{version}/file?path={}&format=html",
        encode(id),
        encode(path)
    )
}

/// `POST /api/reviews/{id}/comments`.
#[must_use]
pub fn comments_url(id: &str) -> String {
    format!("/api/reviews/{}/comments", encode(id))
}

/// `PATCH` and `DELETE /api/reviews/{id}/comments/{number}`.
#[must_use]
pub fn comment_url(id: &str, number: u32) -> String {
    format!("/api/reviews/{}/comments/{number}", encode(id))
}

/// `DELETE /api/reviews/{id}/comments/{number}`, withdrawing `draft`: the
/// comment, or its draft reply.
#[must_use]
pub fn withdraw_url(id: &str, number: u32, draft: Draft) -> String {
    let draft = match draft {
        Draft::Comment => "comment",
        Draft::Reply => "reply",
    };

    format!("{}?draft={draft}", comment_url(id, number))
}

/// `POST /api/reviews/{id}/comments/{number}/replies`.
#[must_use]
pub fn replies_url(id: &str, number: u32) -> String {
    format!("{}/replies", comment_url(id, number))
}

/// `POST /api/reviews/{id}/comments/{number}/resolve`.
#[must_use]
pub fn resolve_url(id: &str, number: u32) -> String {
    format!("{}/resolve", comment_url(id, number))
}

/// `POST /api/reviews/{id}/comments/{number}/reopen`.
#[must_use]
pub fn reopen_comment_url(id: &str, number: u32) -> String {
    format!("{}/reopen", comment_url(id, number))
}

/// `POST /api/reviews/{id}/approvals`.
#[must_use]
pub fn approvals_url(id: &str) -> String {
    format!("/api/reviews/{}/approvals", encode(id))
}

/// `DELETE /api/reviews/{id}/approvals/{number}`.
#[must_use]
pub fn approval_url(id: &str, number: u32) -> String {
    format!("{}/{number}", approvals_url(id))
}

/// `PUT /api/reviews/{id}/read`.
#[must_use]
pub fn read_url(id: &str) -> String {
    format!("/api/reviews/{}/read", encode(id))
}

/// `DELETE /api/reviews/{id}/read` of the file `path`.
#[must_use]
pub fn unread_url(id: &str, path: &str) -> String {
    format!("{}?path={}", read_url(id), encode(path))
}

/// `POST /api/reviews/{id}/reopen`.
#[must_use]
pub fn reopen_url(id: &str) -> String {
    format!("/api/reviews/{}/reopen", encode(id))
}

/// `POST /api/reviews/{id}/passes`.
#[must_use]
pub fn passes_url(id: &str) -> String {
    format!("/api/reviews/{}/passes", encode(id))
}

/// `POST /api/reviews/{id}/stop`.
#[must_use]
pub fn stop_url(id: &str) -> String {
    format!("/api/reviews/{}/stop", encode(id))
}

/// `POST /api/reviews/{id}/versions`.
#[must_use]
pub fn versions_url(id: &str) -> String {
    format!("/api/reviews/{}/versions", encode(id))
}

/// How a request reaches its URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Post,
    Put,
    Patch,
    Delete,
}

impl Method {
    fn as_str(self) -> &'static str {
        match self {
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
        }
    }
}

/// A write to the API: where it goes, how, and the JSON it carries, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub method: Method,
    pub url: String,
    pub body: Option<String>,
}

impl Call {
    /// A `POST` of `body`, as JSON, to `url`.
    #[must_use]
    pub fn post(url: String, body: &impl Serialize) -> Self {
        Self {
            method: Method::Post,
            url,
            body: Some(json(body)),
        }
    }

    /// A `PUT` of `body`, as JSON, to `url`.
    #[must_use]
    pub fn put(url: String, body: &impl Serialize) -> Self {
        Self {
            method: Method::Put,
            url,
            body: Some(json(body)),
        }
    }

    /// A `PATCH` of `body`, as JSON, to `url`.
    #[must_use]
    pub fn patch(url: String, body: &impl Serialize) -> Self {
        Self {
            method: Method::Patch,
            url,
            body: Some(json(body)),
        }
    }

    /// A `POST` without a body.
    #[must_use]
    pub fn post_empty(url: String) -> Self {
        Self {
            method: Method::Post,
            url,
            body: None,
        }
    }

    /// A `DELETE` of `url`.
    #[must_use]
    pub fn delete(url: String) -> Self {
        Self {
            method: Method::Delete,
            url,
            body: None,
        }
    }
}

/// `body` as JSON; empty for a value that can't be written, which the server
/// refuses as unreadable.
fn json(body: &impl Serialize) -> String {
    serde_json::to_string(body).unwrap_or_default()
}

/// `GET /api/reviews/{id}/diff` of the one file `path`, across `span`.
#[must_use]
pub fn diff_url(id: &str, span: Span, path: &str) -> String {
    let mut url = format!("/api/reviews/{}/diff?path={}", encode(id), encode(path));
    match span {
        Span::Pass(pass) => url.push_str(&format!("&pass={pass}")),
        Span::Versions { from, to } => {
            if let Some(from) = from {
                url.push_str(&format!("&from={from}"));
            }
            if let Some(to) = to {
                url.push_str(&format!("&to={to}"));
            }
        }
    }

    url
}

/// What an answer with `status` and the body `text` says: the value it carries,
/// or why the server refused the request.
pub fn read<T: DeserializeOwned>(status: u16, text: &str) -> Result<T, String> {
    if SUCCESS.contains(&status) {
        return serde_json::from_str(text).map_err(|error| {
            format!("The server answered with what this page can't read: {error}")
        });
    }

    match serde_json::from_str::<ErrorBody>(text) {
        Ok(body) => Err(body.error),
        Err(_) => Err(format!("The server answered {status}.")),
    }
}

/// `GET` `url`, reading what the server answers.
pub async fn get<T: DeserializeOwned>(url: &str) -> Result<T, String> {
    fetch(url, "GET", None).await
}

/// `POST` `body`, as JSON, to `url`, reading what the server answers.
pub async fn post<T: DeserializeOwned>(url: &str, body: &impl Serialize) -> Result<T, String> {
    let body = serde_json::to_string(body)
        .map_err(|error| format!("The request can't be written: {error}"))?;

    fetch(url, "POST", Some(body)).await
}

/// Make `call`, reading what the server answers.
pub async fn send<T: DeserializeOwned>(call: Call) -> Result<T, String> {
    fetch(&call.url, call.method.as_str(), call.body).await
}

async fn fetch<T: DeserializeOwned>(
    url: &str,
    method: &str,
    body: Option<String>,
) -> Result<T, String> {
    let init = RequestInit::new();
    init.set_method(method);
    if let Some(body) = &body {
        init.set_body(&JsValue::from_str(body));
    }
    let request = Request::new_with_str_and_init(url, &init).map_err(|error| message(&error))?;
    if body.is_some() {
        request
            .headers()
            .set("Content-Type", "application/json")
            .map_err(|error| message(&error))?;
    }
    let window = web_sys::window().ok_or("The page has no window to send requests from.")?;

    let response: Response = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|error| format!("The server can't be reached: {}", message(&error)))?
        .dyn_into()
        .map_err(|error| message(&error))?;
    let text = JsFuture::from(response.text().map_err(|error| message(&error))?)
        .await
        .map_err(|error| message(&error))?
        .as_string()
        .unwrap_or_default();

    read(response.status(), &text)
}

/// The message of a JavaScript error.
fn message(error: &JsValue) -> String {
    error
        .dyn_ref::<Error>()
        .map(|error| String::from(error.message()))
        .or_else(|| error.as_string())
        .unwrap_or_else(|| "The browser refused the request.".to_owned())
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
