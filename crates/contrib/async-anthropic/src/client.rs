use std::{pin::Pin, time::Duration};

use backon::{ExponentialBuilder, Retryable as _};
use derive_builder::Builder;
use eventsource_stream::Eventsource as _;
use futures::StreamExt as _;
use reqwest::StatusCode;
use secrecy::ExposeSecret;
use serde::{Serialize, de::DeserializeOwned};
use tokio_stream::Stream;

use crate::{
    bearer,
    errors::{
        AnthropicError, ApiError, ApiErrorEnvelope, UnifiedRateLimit, map_deserialization_error,
    },
    messages::Messages,
    models::Models,
};

const BASE_URL: &str = "https://api.anthropic.com";

/// Main entry point for the Anthropic API
///
/// By default will use the `ANTHROPIC_API_KEY` environment variable
///
/// # Example
///
/// ```no_run
/// # use async_anthropic::types::*;
/// # async fn run() {
/// let client = async_anthropic::Client::default();
///
/// let request = CreateMessagesRequestBuilder::default()
///     .model("claude-3.5-sonnet")
///     .messages(vec![
///         MessageBuilder::default()
///             .role(MessageRole::User)
///             .content("Hello world!")
///             .build()
///             .unwrap(),
///     ])
///     .build()
///     .unwrap();
///
/// client.messages().create(request).await.unwrap();
/// # }
/// ```
#[derive(Clone, Debug, Builder)]
#[builder(setter(into, strip_option))]
pub struct Client {
    #[builder(default)]
    http_client: reqwest::Client,
    #[builder(default)]
    base_url: String,
    #[builder(default = default_api_key())]
    api_key: secrecy::SecretString,
    /// OAuth bearer token for subscription (Claude Pro/Max) accounts.
    ///
    /// When set, requests authenticate with `Authorization: Bearer <token>` and
    /// carry the Claude Code request fingerprint (see [`bearer`]); `api_key` is
    /// ignored and `x-api-key` is never sent.
    /// User-configured `beta` values merge after the fingerprint's own beta set
    /// and can neither remove nor duplicate its entries.
    #[builder(default)]
    auth_token: Option<secrecy::SecretString>,
    #[builder(default)]
    version: String,
    #[builder(default)]
    beta: Option<String>,
    #[builder(default)]
    backoff: ExponentialBuilder,
}

impl Default for Client {
    fn default() -> Self {
        // Load backoff settings from configuration
        let backoff = ExponentialBuilder::default()
            .with_min_delay(Duration::from_secs(15))
            .with_factor(2.0)
            .with_jitter()
            .with_max_delay(Duration::from_mins(2));

        Self {
            http_client: reqwest::Client::new(),
            api_key: default_api_key(), // Default env?
            auth_token: None,
            version: "2023-06-01".to_string(),
            beta: None,
            base_url: BASE_URL.to_string(),
            backoff,
        }
    }
}

fn default_api_key() -> secrecy::SecretString {
    if cfg!(test) {
        return "test".into();
    }
    std::env::var("ANTHROPIC_API_KEY")
        .unwrap_or_else(|_| {
            tracing::warn!("Default Anthropic client initialized without api key");
            String::new()
        })
        .into()
}

impl Client {
    /// Build a new client from an API key
    pub fn from_api_key(api_key: impl Into<secrecy::SecretString>) -> Self {
        Self {
            api_key: api_key.into(),
            ..Default::default()
        }
    }

    /// Create a new client builder
    #[must_use]
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }

    /// Set a custom backoff strategy
    #[must_use]
    pub fn with_backoff(mut self, backoff: ExponentialBuilder) -> Self {
        self.backoff = backoff;
        self
    }

    /// Call the messages api
    #[must_use]
    pub fn messages(&self) -> Messages<'_> {
        Messages::new(self)
    }

    #[must_use]
    pub fn models(&self) -> Models<'_> {
        Models::new(self)
    }

    /// Build the request headers, folding `betas` into the client's own set.
    ///
    /// `betas` are the extras a single request asked for; they join the
    /// client-wide `beta` value in order, without duplicates.
    fn headers(&self, betas: &[String]) -> reqwest::header::HeaderMap {
        let mut extras: Vec<&str> = self
            .beta
            .iter()
            .flat_map(|beta| beta.split(','))
            .map(str::trim)
            .filter(|beta| !beta.is_empty())
            .collect();

        for beta in betas {
            let beta = beta.trim();
            if !beta.is_empty() && !extras.contains(&beta) {
                extras.push(beta);
            }
        }

        let extras = extras.join(",");
        let extras = (!extras.is_empty()).then_some(extras);

        if let Some(token) = &self.auth_token {
            // The fingerprint's own betas lead and can be neither removed nor
            // duplicated by these; see `bearer::merge_betas`.
            return bearer::headers(token.expose_secret(), &self.version, extras.as_deref());
        }

        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-api-key", self.api_key.expose_secret().parse().unwrap());
        headers.insert("anthropic-version", self.version.parse().unwrap());
        if let Some(extras) = extras {
            headers.insert("anthropic-beta", extras.parse().unwrap());
        }

        headers
    }

    fn format_url(&self, path: &str) -> String {
        format!(
            "{}/{}",
            &self.base_url.trim_end_matches('/'),
            &path.trim_start_matches('/')
        )
    }

    pub async fn get<O>(&self, path: &str) -> Result<O, AnthropicError>
    where
        O: DeserializeOwned,
    {
        let request = || async {
            let response = self
                .http_client
                .get(self.format_url(path))
                .headers(self.headers(&[]))
                .send()
                .await
                .map_err(AnthropicError::Network)?;

            handle_response(response).await
        };

        request
            .retry(self.backoff)
            .sleep(tokio::time::sleep)
            // A spent quota is not throttling: waiting cannot help, and the
            // caller may have another credential that can serve the request
            // now. Only capacity throttling is retried in place.
            .when(|e| {
                matches!(e, AnthropicError::RateLimit { limits, .. } if !limits.is_quota_rejection())
            })
            .adjust(|err, dur| match err {
                AnthropicError::RateLimit { retry_after, .. } => {
                    retry_after.map(Duration::from_secs).or(dur)
                }
                _ => dur,
            })
            .await
    }

    /// Make post request to the API
    ///
    /// This includes all headers and error handling
    pub async fn post<I, O>(
        &self,
        path: &str,
        request: I,
        betas: &[String],
    ) -> Result<O, AnthropicError>
    where
        I: Serialize,
        O: DeserializeOwned,
    {
        let request = || async {
            // `headers()` already carries the `anthropic-beta` value.
            let request = self
                .http_client
                .post(self.format_url(path))
                .headers(self.headers(betas))
                .json(&request);

            let response = request.send().await.map_err(AnthropicError::Network)?;

            handle_response(response).await
        };

        request
            .retry(self.backoff)
            .sleep(tokio::time::sleep)
            // A spent quota is not throttling: waiting cannot help, and the
            // caller may have another credential that can serve the request
            // now. Only capacity throttling is retried in place.
            .when(|e| {
                matches!(e, AnthropicError::RateLimit { limits, .. } if !limits.is_quota_rejection())
            })
            .adjust(|err, dur| match err {
                AnthropicError::RateLimit { retry_after, .. } => {
                    retry_after.map(Duration::from_secs).or(dur)
                }
                _ => dur,
            })
            .await
    }

    /// Open a server-sent-event stream.
    ///
    /// Returns the response's quota headers alongside the event stream: the
    /// unified rate-limit headers ride on every response, including successful
    /// ones, and are the only place a subscription's usage state is reported.
    ///
    /// The stream is not reconnected on failure.
    /// Anthropic's message endpoint sends no SSE event ids, so a reconnect
    /// cannot resume: it would issue a fresh completion whose content restarts
    /// from the beginning.
    /// Recovery belongs to the caller, which can rebuild the request from
    /// whatever it already received.
    pub(crate) async fn post_stream<I, O, const N: usize>(
        &self,
        path: &str,
        request: I,
        event_types: [&'static str; N],
        betas: &[String],
    ) -> Result<StreamResponse<O>, AnthropicError>
    where
        I: Serialize,
        O: DeserializeOwned + Send + 'static,
    {
        let response = self
            .http_client
            .post(self.format_url(path))
            .headers(self.headers(betas))
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .json(&request)
            .send()
            .await
            .map_err(AnthropicError::Network)?;

        // Read the quota headers before the body is consumed, on success as
        // well as failure.
        let limits = UnifiedRateLimit::from_headers(response.headers());

        if response.status() != StatusCode::OK {
            return Err(response_error(response).await);
        }

        Ok(StreamResponse {
            limits,
            stream: stream(response, event_types),
        })
    }
}

/// A server-sent-event stream plus the quota state its response reported.
pub struct StreamResponse<O> {
    /// The unified rate-limit headers the response carried.
    pub limits: UnifiedRateLimit,

    /// The parsed events.
    pub stream: Pin<Box<dyn Stream<Item = Result<O, AnthropicError>> + Send>>,
}

async fn handle_response<O>(response: reqwest::Response) -> Result<O, AnthropicError>
where
    O: DeserializeOwned,
{
    if response.status() == StatusCode::OK {
        return response.json::<O>().await.map_err(AnthropicError::Network);
    }

    Err(response_error(response).await)
}

/// Classify a non-success response.
///
/// Shared by the buffered and streaming paths so one place decides what a
/// status means.
async fn response_error(response: reqwest::Response) -> AnthropicError {
    let status = response.status();

    // 529 is the status code for overloaded requests
    let overloaded_status = StatusCode::from_u16(529).expect("529 is a valid status code");

    if status == StatusCode::TOO_MANY_REQUESTS || status == overloaded_status {
        let retry_after = response
            .headers()
            .get("Retry-After")
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok());

        // Read the quota headers before consuming the body: they are
        // the only thing that distinguishes a spent usage window from
        // capacity throttling, and the body often says nothing.
        let limits = UnifiedRateLimit::from_headers(response.headers());

        let text = response.text().await.unwrap_or_default();
        tracing::warn!(?limits, "Rate limited: {text}");

        return AnthropicError::RateLimit {
            retry_after,
            limits,
        };
    }

    // The credential was refused. The status is what makes this actionable,
    // and it is lost once the body is parsed into the generic error envelope.
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        let text = response.text().await.unwrap_or_default();
        tracing::warn!(%status, "Authentication rejected: {text}");

        let error = serde_json::from_str::<ApiErrorEnvelope>(&text).map_or_else(
            |_| ApiError {
                error_type: "authentication_error".to_owned(),
                message: Some(text),
            },
            |envelope| envelope.error,
        );

        return AnthropicError::Auth {
            status: status.as_u16(),
            error,
        };
    }

    let text = response.text().await.unwrap_or_default();
    match serde_json::from_str::<ApiErrorEnvelope>(&text) {
        Ok(envelope) => AnthropicError::Api(envelope.error),
        Err(_) => AnthropicError::Unknown(text),
    }
}

/// Parse an SSE response body into typed events.
///
/// The stream is driven by the caller rather than by a background task, so a
/// dropped stream stops reading the body instead of continuing to consume it.
fn stream<O, const N: usize>(
    response: reqwest::Response,
    event_types: [&'static str; N],
) -> Pin<Box<dyn Stream<Item = Result<O, AnthropicError>> + Send>>
where
    O: DeserializeOwned + Send + 'static,
{
    let events = response
        .bytes_stream()
        .eventsource()
        .map(move |event| {
            tracing::trace!("Streaming event: {event:?}");
            match event {
                Ok(message) => decode_event(&message, &event_types),
                Err(error) => Err(AnthropicError::StreamTransport(error.to_string())),
            }
        })
        // A failure ends the stream: whatever follows belongs to a response
        // the caller has already been told is broken.
        .scan(false, |finished, item| {
            if *finished {
                return futures::future::ready(None);
            }

            *finished = item.is_err();
            futures::future::ready(Some(item))
        });

    Box::pin(events)
}

/// Decode one server-sent event into `O`.
///
/// An `error` event carries an API error rather than a payload; an event whose
/// type the caller did not ask for is a protocol surprise rather than data.
fn decode_event<O, const N: usize>(
    message: &eventsource_stream::Event,
    event_types: &[&'static str; N],
) -> Result<O, AnthropicError>
where
    O: DeserializeOwned,
{
    let event = message.event.as_str();

    if event == "error" {
        return Err(
            match serde_json::from_str::<ApiErrorEnvelope>(&message.data) {
                Ok(envelope) => AnthropicError::Api(envelope.error),
                Err(_) => match serde_json::from_str::<ApiError>(&message.data) {
                    Ok(error) => AnthropicError::Api(error),
                    Err(error) => map_deserialization_error(error, message.data.as_bytes()),
                },
            },
        );
    }

    if !event_types.contains(&event) {
        return Err(AnthropicError::StreamTransport(format!(
            "unknown event type: {event}"
        )));
    }

    serde_json::from_str::<O>(&message.data)
        .map_err(|error| map_deserialization_error(error, message.data.as_bytes()))
}
