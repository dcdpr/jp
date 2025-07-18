use std::{collections::HashMap, io, pin::Pin, time::Duration};

use async_stream::stream;
use backon::{ExponentialBuilder, Retryable as _};
use futures::{Stream, StreamExt as _, TryStreamExt as _};
use reqwest::{
    header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE, REFERER},
    RequestBuilder,
};
use serde_json::Value;
use tokio_util::{
    codec::{FramedRead, LinesCodec},
    io::StreamReader,
};
use tracing::{error, trace, warn};

use crate::{
    error::{Error, Result},
    types::{
        request,
        response::{self, ChatCompletionError, ErrorResponse},
    },
};

#[derive(Debug, Clone)]
pub struct Client {
    pub api_key: String,
    pub app_name: Option<String>,
    pub app_referrer: Option<String>,
    http_client: reqwest::Client,
    base_url: String,
}

impl Client {
    #[must_use]
    pub fn new(api_key: String, app_name: Option<String>, app_referrer: Option<String>) -> Self {
        Self {
            api_key,
            app_name,
            app_referrer,
            http_client: reqwest::Client::new(),
            base_url: "https://openrouter.ai".to_string(),
        }
    }

    #[must_use]
    pub fn with_base_url(mut self, base_url: String) -> Self {
        self.base_url = base_url;
        self
    }

    /// Build HTTP headers required for making API calls.
    /// Returns an error if any header value cannot be constructed.
    fn build_headers(&self) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        headers.insert(
            AUTHORIZATION,
            format!("Bearer {}", self.api_key)
                .parse()
                .map_err(|e| Error::Config(format!("Invalid API key header format: {e}")))?,
        );

        if let Some(referer) = &self.app_referrer {
            headers.insert(
                REFERER,
                referer
                    .parse()
                    .map_err(|e| Error::Config(format!("Invalid Referer header: {e}")))?,
            );
        }

        if let Some(title) = &self.app_name {
            headers.insert(
                "X-Title",
                title
                    .parse()
                    .map_err(|e| Error::Config(format!("Invalid Title header: {e}")))?,
            );
        }

        Ok(headers)
    }

    pub async fn models(&self) -> Result<response::ModelsResponse> {
        let url = format!("{}/api/v1/models", self.base_url);
        let headers = self.build_headers()?;
        let request = self.http_client.get(url).headers(headers);

        Ok(get(request)
            .await?
            .json::<response::ModelsResponse>()
            .await?)
    }

    #[must_use]
    pub fn chat_completion_stream(
        &self,
        request: request::ChatCompletion,
    ) -> Pin<Box<dyn Stream<Item = Result<response::ChatCompletion>> + Send>> {
        let client = self.clone();

        let backoff = ExponentialBuilder::default()
            .with_min_delay(Duration::from_millis(10))
            .with_max_delay(Duration::from_secs(5))
            .with_total_delay(Some(Duration::from_secs(10)));

        trace!("Request retry configured.");

        let retry_stream = stream! {
            let operation = || async {
                client
                    .chat_completion_stream_inner(request.clone())
                    .await
            };

            let stream = operation
                .retry(backoff)
                .when(is_transient_error)
                .notify(|error, backoff| warn!(?error, ?backoff, "Request failed. Retrying."))
                .await?;

            tokio::pin!(stream);
            while let Some(item) = stream.next().await {
                yield item;
            }
        };

        Box::pin(retry_stream)
    }

    async fn chat_completion_stream_inner(
        &self,
        request: request::ChatCompletion,
    ) -> Result<impl Stream<Item = Result<response::ChatCompletion>>> {
        let request = self.prepare_request(request, true)?;
        let response = get(request).await?;
        let byte_stream = response.bytes_stream().map_err(io::Error::other);
        let lines = FramedRead::new(StreamReader::new(byte_stream), LinesCodec::new());

        // Transform the lines stream into a completion chunk stream
        let chunk_stream = lines
            .map_err(|e| Error::Stream(format!("Stream error: {e}")))
            .filter_map(|line_result| async move {
                match line_result {
                    Ok(line) => {
                        if line.trim().is_empty() {
                            return None;
                        }

                        if !line.starts_with("data:") {
                            return None;
                        }

                        // Each data line starts with "data:".
                        let data_part = line.trim_start_matches("data:").trim();

                        // Marks the end of the Openrouter SSE stream.
                        //
                        // See: <https://openrouter.ai/docs/api-reference/streaming>
                        if data_part == "[DONE]" {
                            return None;
                        }

                        Some(parse_chunk(data_part))
                    }
                    Err(e) => Some(Err(e)),
                }
            });

        Ok(chunk_stream)
    }

    pub async fn chat_completion(
        &self,
        request: request::ChatCompletion,
    ) -> Result<response::ChatCompletion> {
        let request = self.prepare_request(request, false)?;
        let text = get(request).await?.text().await?;
        parse_chunk(&text)
    }

    fn prepare_request(
        &self,
        request: request::ChatCompletion,
        stream: bool,
    ) -> Result<RequestBuilder> {
        let url = format!("{}/api/v1/chat/completions", self.base_url);
        let mut body = serde_json::to_value(request).map_err(|e| Error::Api {
            code: 500,
            message: format!("Request serialization error: {e}"),
        })?;

        if stream {
            body["stream"] = Value::Bool(true);
        }

        let headers = self.build_headers()?;
        let redacted_headers = headers
            .iter()
            .map(|(k, v)| {
                if k.as_str() == AUTHORIZATION {
                    return (k.to_owned(), "[REDACTED]".to_string());
                }

                (k.to_owned(), v.to_str().unwrap_or_default().to_owned())
            })
            .collect::<HashMap<_, _>>();

        trace!(%url, headers = ?redacted_headers, "Triggering request.");

        Ok(self.http_client.post(&url).headers(headers).json(&body))
    }
}

async fn get(request: RequestBuilder) -> Result<reqwest::Response> {
    let response = request.send().await?;

    trace!(
        status = response.status().as_u16(),
        content_length = response.content_length().unwrap_or_default(),
        content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .map(|v| v.to_str().unwrap_or_default()),
        "Received response."
    );

    let status = response.status();
    if status.is_client_error() || status.is_server_error() {
        let status = status.as_u16();
        let body = response.text().await?;

        error!(status, body, "Unexpected response.");

        return Err(Error::Api {
            code: status,
            message: body,
        });
    }

    Ok(response)
}

fn parse_chunk(chunk: &str) -> Result<response::ChatCompletion> {
    use serde_json::{from_str, to_string_pretty};

    let json_error = match from_str(chunk) {
        Ok(response) => return Ok(response),
        Err(error) => error,
    };

    let Ok(ChatCompletionError { error, .. }) = from_str::<ChatCompletionError>(chunk) else {
        return Err(Error::Json(json_error));
    };

    let ErrorResponse {
        code,
        message,
        metadata,
    } = error;

    let details = metadata
        .map(|metadata| match metadata {
            response::ErrorMetadata::Moderation {
                reasons,
                provider_name,
                ..
            } => format!(": ({provider_name}) {}", reasons.join("\n")),
            response::ErrorMetadata::Provider { provider_name, raw } => {
                let json = to_string_pretty(&raw).unwrap_or_default();
                format!(": ({provider_name}) {json}")
            }
        })
        .unwrap_or_default();

    Err(Error::Api {
        code,
        message: format!("{message}{details}"),
    })
}

// Helper function to determine if an error is transient (retryable)
fn is_transient_error(err: &Error) -> bool {
    match err {
        Error::Request(req_err) => req_err.is_timeout() || req_err.is_connect(),
        Error::Api { code, .. } => matches!(code, 408 | 429 | 500 | 502 | 503 | 504),
        Error::Stream(_) => true, // Retry on stream processing errors
        Error::Config(_) | Error::Json(_) | Error::Io(_) => false,
    }
}
