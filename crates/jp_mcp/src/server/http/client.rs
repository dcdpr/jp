//! The MCP Host's HTTP client for the loopback endpoint.
//!
//! rmcp ships a Streamable HTTP client for `reqwest` 0.13, while the rest of JP
//! uses 0.12.
//! This implements rmcp's [`StreamableHttpClient`] over the `reqwest` JP
//! already depends on, so the Host's connection to its own endpoint does not
//! pull in a second HTTP stack.
//!
//! It talks to one server, the endpoint in the same process, which sends no
//! `WWW-Authenticate` challenges: a `401` or `403` is reported as an unexpected
//! response rather than as an authorization flow.

use std::{borrow::Cow, collections::HashMap, sync::Arc};

use futures::{StreamExt as _, stream::BoxStream};
use reqwest::{
    Client, RequestBuilder, StatusCode,
    header::{ACCEPT, CONTENT_TYPE, HeaderName, HeaderValue},
};
use rmcp::{
    model::{ClientJsonRpcMessage, JsonRpcMessage, ServerJsonRpcMessage},
    transport::{
        common::http_header::{
            EVENT_STREAM_MIME_TYPE, HEADER_LAST_EVENT_ID, HEADER_SESSION_ID, JSON_MIME_TYPE,
        },
        streamable_http_client::{
            SseError, StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
        },
    },
};
use sse_stream::{Sse, SseStream};
use tracing::warn;

type Error = StreamableHttpError<reqwest::Error>;

/// A `reqwest` client speaking MCP Streamable HTTP.
#[derive(Debug, Clone)]
pub(super) struct LoopbackClient(pub(super) Client);

/// Add the headers rmcp's worker supplies to every request.
fn with_headers(
    builder: RequestBuilder,
    auth_token: Option<String>,
    custom_headers: HashMap<HeaderName, HeaderValue>,
) -> RequestBuilder {
    // rmcp's worker uses custom headers only to carry the negotiated
    // `MCP-Protocol-Version`; the Host configures none of its own.
    let builder = custom_headers
        .into_iter()
        .fold(builder, |builder, (name, value)| {
            builder.header(name, value)
        });

    match auth_token {
        Some(token) => builder.bearer_auth(token),
        None => builder,
    }
}

impl StreamableHttpClient for LoopbackClient {
    type Error = reqwest::Error;

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        last_event_id: Option<String>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, Error> {
        let mut builder = self
            .0
            .get(uri.as_ref())
            .header(ACCEPT, accept())
            .header(HEADER_SESSION_ID, session_id.as_ref());
        if let Some(last_event_id) = last_event_id {
            builder = builder.header(HEADER_LAST_EVENT_ID, last_event_id);
        }

        let response = with_headers(builder, auth_token, custom_headers)
            .send()
            .await
            .map_err(Error::Client)?;

        if response.status() == StatusCode::METHOD_NOT_ALLOWED {
            return Err(Error::ServerDoesNotSupportSse);
        }
        let response = response.error_for_status().map_err(Error::Client)?;

        match content_type(&response) {
            Some(ct) if is(&ct, EVENT_STREAM_MIME_TYPE) || is(&ct, JSON_MIME_TYPE) => {}
            other => return Err(Error::UnexpectedContentType(other)),
        }

        Ok(SseStream::from_bytes_stream(response.bytes_stream()).boxed())
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), Error> {
        let builder = self
            .0
            .delete(uri.as_ref())
            .header(HEADER_SESSION_ID, session_id.as_ref());

        let response = with_headers(builder, auth_token, custom_headers)
            .send()
            .await
            .map_err(Error::Client)?;

        if response.status() == StatusCode::METHOD_NOT_ALLOWED {
            return Ok(());
        }
        response.error_for_status().map_err(Error::Client)?;

        Ok(())
    }

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_token: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, Error> {
        let body = serde_json::to_vec(&message)?;
        let mut builder = self
            .0
            .post(uri.as_ref())
            .header(ACCEPT, accept())
            .header(CONTENT_TYPE, JSON_MIME_TYPE)
            .body(body);
        let session_was_attached = session_id.is_some();
        if let Some(session_id) = session_id {
            builder = builder.header(HEADER_SESSION_ID, session_id.as_ref());
        }

        let response = with_headers(builder, auth_token, custom_headers)
            .send()
            .await
            .map_err(Error::Client)?;

        let status = response.status();
        if matches!(status, StatusCode::ACCEPTED | StatusCode::NO_CONTENT) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        if status == StatusCode::NOT_FOUND && session_was_attached {
            return Err(Error::SessionExpired);
        }

        let content_type = content_type(&response);
        let session_id = response
            .headers()
            .get(HEADER_SESSION_ID)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);

        // The spec answers notifications and responses with `202`, but an
        // empty `200` means the same thing.
        if status.is_success()
            && response.content_length() == Some(0)
            && !matches!(message, ClientJsonRpcMessage::Request(_))
        {
            return Ok(StreamableHttpPostResponse::Accepted);
        }

        // A failure status can still carry a JSON-RPC error, which the caller
        // should see as the MCP error it is rather than a transport failure.
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            if content_type
                .as_deref()
                .is_some_and(|ct| is(ct, JSON_MIME_TYPE))
                && let Some(error) = json_rpc_error(&body)
            {
                return Ok(StreamableHttpPostResponse::Json(error, session_id));
            }

            return Err(Error::UnexpectedServerResponse(Cow::Owned(format!(
                "HTTP {status}: {body}"
            ))));
        }

        match content_type.as_deref() {
            Some(ct) if is(ct, EVENT_STREAM_MIME_TYPE) => {
                let stream = SseStream::from_bytes_stream(response.bytes_stream()).boxed();
                Ok(StreamableHttpPostResponse::Sse(stream, session_id))
            }
            Some(ct) if is(ct, JSON_MIME_TYPE) => {
                let body = response.bytes().await.map_err(Error::Client)?;
                match serde_json::from_slice::<ServerJsonRpcMessage>(&body) {
                    Ok(message) => Ok(StreamableHttpPostResponse::Json(message, session_id)),
                    Err(error) => {
                        warn!(%error, "Unparseable JSON-RPC response; treating it as accepted.");
                        Ok(StreamableHttpPostResponse::Accepted)
                    }
                }
            }
            _ => Err(Error::UnexpectedContentType(content_type)),
        }
    }
}

/// The `Accept` value every MCP request carries.
fn accept() -> String {
    format!("{EVENT_STREAM_MIME_TYPE}, {JSON_MIME_TYPE}")
}

fn content_type(response: &reqwest::Response) -> Option<String> {
    response
        .headers()
        .get(CONTENT_TYPE)
        .map(|ct| String::from_utf8_lossy(ct.as_bytes()).into_owned())
}

/// Whether a `Content-Type` value names `mime`, ignoring any parameters.
fn is(content_type: &str, mime: &str) -> bool {
    content_type.starts_with(mime)
}

/// `body` as a JSON-RPC error, when it is one.
fn json_rpc_error(body: &str) -> Option<ServerJsonRpcMessage> {
    match serde_json::from_str::<ServerJsonRpcMessage>(body) {
        Ok(message @ JsonRpcMessage::Error(_)) => Some(message),
        _ => None,
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
