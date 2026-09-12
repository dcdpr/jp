//! Reqwest transport for the MCP Host's connection to its own HTTP endpoint.
//!
//! MCP session handling, cancellation, and SSE resumption belong to rmcp's
//! transport worker.
//! This adapter sends HTTP requests and decodes responses.

use std::{collections::HashMap, sync::Arc};

use futures::{StreamExt as _, stream::BoxStream};
use reqwest::{
    Client, Error, RequestBuilder, Response, StatusCode,
    header::{ACCEPT, CONTENT_TYPE, HeaderName, HeaderValue},
    redirect::Policy,
};
use rmcp::{
    model::ClientJsonRpcMessage,
    transport::streamable_http_client::{
        StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
    },
};
use sse_stream::{Error as SseError, Sse, SseStream};

/// HTTP client for JP's private loopback connection, without proxies or
/// redirects.
#[derive(Clone)]
pub(super) struct LoopbackClient(Client);

impl LoopbackClient {
    /// Construct a client that cannot route the local connection through a
    /// proxy.
    pub(super) fn new() -> Result<Self, Error> {
        Ok(Self(
            Client::builder()
                .no_proxy()
                .redirect(Policy::none())
                .build()?,
        ))
    }
}

fn request_headers(
    mut request: RequestBuilder,
    session: Option<&str>,
    auth: Option<String>,
    headers: HashMap<HeaderName, HeaderValue>,
) -> Result<RequestBuilder, StreamableHttpError<Error>> {
    for (name, value) in headers {
        if matches!(
            name.as_str(),
            "accept"
                | "content-type"
                | "mcp-session-id"
                | "last-event-id"
                | "authorization"
                | "host"
        ) {
            return Err(StreamableHttpError::ReservedHeaderConflict(
                name.to_string(),
            ));
        }
        request = request.header(name, value);
    }
    if let Some(session) = session {
        request = request.header("mcp-session-id", session);
    }
    if let Some(auth) = auth {
        request = request.bearer_auth(auth);
    }
    Ok(request)
}

fn content_type(response: &Response) -> Result<&str, StreamableHttpError<Error>> {
    let value = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok());
    value
        .map(|value| value.split(';').next().unwrap_or(value).trim())
        .ok_or(StreamableHttpError::UnexpectedContentType(None))
}

fn event_stream(response: Response) -> BoxStream<'static, Result<Sse, SseError>> {
    SseStream::from_bytes_stream(response.bytes_stream()).boxed()
}

impl StreamableHttpClient for LoopbackClient {
    type Error = Error;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, StreamableHttpError<Error>> {
        let request = self
            .0
            .post(uri.as_ref())
            .header(ACCEPT, "application/json, text/event-stream")
            .json(&message);
        let response =
            request_headers(request, session_id.as_deref(), auth_header, custom_headers)?
                .send()
                .await
                .map_err(StreamableHttpError::Client)?;
        if response.status() == StatusCode::NOT_FOUND && session_id.is_some() {
            return Err(StreamableHttpError::SessionExpired);
        }
        let response = response
            .error_for_status()
            .map_err(StreamableHttpError::Client)?;
        if matches!(
            response.status(),
            StatusCode::ACCEPTED | StatusCode::NO_CONTENT
        ) {
            return Ok(StreamableHttpPostResponse::Accepted);
        }
        let session = response
            .headers()
            .get("mcp-session-id")
            .map(|value| value.to_str().map(str::to_owned))
            .transpose()
            .map_err(|_| {
                StreamableHttpError::UnexpectedServerResponse("invalid MCP session header".into())
            })?;
        match content_type(&response)? {
            "application/json" => Ok(StreamableHttpPostResponse::Json(
                response.json().await.map_err(StreamableHttpError::Client)?,
                session,
            )),
            "text/event-stream" => Ok(StreamableHttpPostResponse::Sse(
                event_stream(response),
                session,
            )),
            other => Err(StreamableHttpError::UnexpectedContentType(Some(
                other.into(),
            ))),
        }
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), StreamableHttpError<Error>> {
        let request = self.0.delete(uri.as_ref());
        let response = request_headers(request, Some(&session_id), auth_header, custom_headers)?
            .send()
            .await
            .map_err(StreamableHttpError::Client)?;
        if response.status() == StatusCode::METHOD_NOT_ALLOWED {
            return Err(StreamableHttpError::ServerDoesNotSupportDeleteSession);
        }
        response
            .error_for_status()
            .map_err(StreamableHttpError::Client)?;
        Ok(())
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        last_event_id: Option<String>,
        auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxStream<'static, Result<Sse, SseError>>, StreamableHttpError<Error>> {
        let mut request = self.0.get(uri.as_ref()).header(ACCEPT, "text/event-stream");
        if let Some(id) = last_event_id {
            request = request.header("last-event-id", id);
        }
        let response = request_headers(request, Some(&session_id), auth_header, custom_headers)?
            .send()
            .await
            .map_err(StreamableHttpError::Client)?;
        match response.status() {
            StatusCode::METHOD_NOT_ALLOWED => {
                return Err(StreamableHttpError::ServerDoesNotSupportSse);
            }
            StatusCode::NOT_FOUND => return Err(StreamableHttpError::SessionExpired),
            _ => {}
        }
        let response = response
            .error_for_status()
            .map_err(StreamableHttpError::Client)?;
        if content_type(&response)? != "text/event-stream" {
            return Err(StreamableHttpError::UnexpectedContentType(Some(
                content_type(&response)?.into(),
            )));
        }
        Ok(event_stream(response))
    }
}

#[cfg(test)]
#[path = "http_client_tests.rs"]
mod tests;
