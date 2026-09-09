//! Anthropic's Message Batches API, driven one request at a time.
//!
//! A batch holds a single request, is polled until it ends, and its one result
//! is replayed as the stream events the synchronous endpoint would have
//! produced.
//! This is what `assistant.model.parameters.service_tier = "flex"` resolves to
//! on Anthropic: half price, no token streaming, and a wait measured in minutes
//! rather than seconds.
//!
//! See:
//! <https://platform.claude.com/docs/en/build-with-claude/batch-processing>

use std::{
    mem,
    time::{Duration, Instant},
};

use async_anthropic::{
    Client,
    errors::{AnthropicError, ApiError},
    types,
};
use async_stream::try_stream;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio::time::sleep;
use tracing::{debug, info, warn};

use crate::{error::StreamError, event::Event, stream::EventStream};

/// The `custom_id` carried by the single request in every batch JP submits.
///
/// Anthropic requires the id to match `^[a-zA-Z0-9_-]{1,64}$` and uses it to
/// pair results with requests; with one request per batch there is nothing to
/// disambiguate.
const CUSTOM_ID: &str = "jp";

/// Where the Message Batches API lives, relative to the configured base URL.
const BATCHES_PATH: &str = "/v1/messages/batches";

/// How often a keep-alive is emitted while the batch is being waited on.
///
/// The downstream idle timeout treats a silent stream as a dead connection and
/// rebuilds it, which for a batch means submitting and paying for a second one.
/// Five seconds stays below the enforced minimum `stream_idle_timeout_secs`
/// (10s), so a heartbeat always lands before the idle window elapses,
/// regardless of how far apart the polls are.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

/// The shortest gap allowed between two status checks.
///
/// A zero-second interval would poll in a tight loop and exhaust the Batches
/// API's own request rate limit long before the batch finished.
pub(super) const MIN_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// How many consecutive transport failures a wait tolerates before giving up.
///
/// A hiccup while polling must not end a wait measured in minutes: the batch
/// keeps running either way, and surfacing the error hands the caller something
/// its retry layer answers by submitting a second, separately billed batch.
/// The counter resets on every successful poll, so only a sustained outage
/// exhausts it.
const MAX_TRANSPORT_FAILURES: u8 = 5;

/// How long to wait between polls, and how long to keep waiting overall.
#[derive(Debug, Clone, Copy)]
pub(super) struct PollConfig {
    /// How long to wait between status checks.
    pub interval: Duration,

    /// How long to wait in total before giving up on the batch.
    ///
    /// `None` waits as long as Anthropic keeps the batch alive, which is 24
    /// hours.
    pub max_wait: Option<Duration>,
}

/// Submit `request` as a single-request batch and stream its result.
///
/// The batch is created before the first poll, so a rejected submission
/// surfaces as the first item of the stream rather than silently later.
/// While the batch runs, the stream carries nothing but keep-alives; the whole
/// response arrives at once when it ends.
///
/// Abandoning the returned stream cancels the batch.
pub(super) fn events(
    client: Client,
    request: types::CreateMessagesRequest,
    is_structured: bool,
    poll: PollConfig,
) -> EventStream {
    Box::pin(try_stream!({
        let mut request = request;
        let betas = mem::take(&mut request.betas);

        let batch = create(&client, &request, &betas)
            .await
            .map_err(StreamError::from)?;

        info!(
            batch = %batch.id,
            interval_secs = poll.interval.as_secs(),
            "Submitted Anthropic message batch."
        );

        // Armed for as long as the batch is running: dropping the stream is how
        // the caller abandons a turn, and a batch nobody is waiting for still
        // bills for whatever the model has already produced.
        let mut cancel_guard = CancelOnDrop {
            client: client.clone(),
            id: Some(batch.id.clone()),
        };

        let id = batch.id;
        let started = Instant::now();
        let mut status = batch.processing_status;
        let mut failures = 0;

        while status != ProcessingStatus::Ended {
            if let Some(max_wait) = poll.max_wait
                && started.elapsed() >= max_wait
            {
                // The batch outlives this process: report where to find it
                // rather than cancelling work that is already paid for.
                cancel_guard.disarm();
                Err(StreamError::other(format!(
                    "Anthropic batch {id} has not finished after {}s and is still running. \
                     Retrieve it with `GET {BATCHES_PATH}/{id}`, or raise \
                     `providers.llm.anthropic.batch_max_wait_secs`.",
                    max_wait.as_secs()
                )))?;
            }

            let detail = match status {
                ProcessingStatus::Canceling => "cancelling the batch",
                _ => "waiting on the batch",
            };

            let mut remaining = poll.interval.max(MIN_POLL_INTERVAL);
            while !remaining.is_zero() {
                let slice = HEARTBEAT_INTERVAL.min(remaining);
                sleep(slice).await;
                remaining = remaining.saturating_sub(slice);
                yield Event::keep_alive_with_detail(detail);
            }

            match retrieve(&client, &id).await {
                Ok(batch) => {
                    failures = 0;
                    status = batch.processing_status;
                    debug!(
                        batch = %id,
                        ?status,
                        elapsed_secs = started.elapsed().as_secs(),
                        "Polled batch."
                    );
                }
                Err(error) => {
                    let error = StreamError::from(error);
                    failures += 1;
                    if !error.is_retryable() || failures > MAX_TRANSPORT_FAILURES {
                        Err(error)?;
                    } else {
                        warn!(
                            batch = %id,
                            failures,
                            %error,
                            "Failed to poll the batch; still waiting."
                        );
                    }
                }
            }
        }

        // An ended batch has nothing left to cancel.
        cancel_guard.disarm();

        info!(
            batch = %id,
            elapsed_secs = started.elapsed().as_secs(),
            "Anthropic message batch finished."
        );

        // The answer is stored for 29 days and is already paid for, so a
        // transport failure here is worth waiting out rather than reporting:
        // the caller's retry would submit a whole new batch to fetch it.
        let outcome = loop {
            match result(&client, &id).await {
                Ok(outcome) => break outcome,
                Err(error) => {
                    let error = StreamError::from(error);
                    failures += 1;
                    if !error.is_retryable() || failures > MAX_TRANSPORT_FAILURES {
                        Err(error)?;
                    } else {
                        warn!(batch = %id, failures, %error, "Failed to read the batch result.");
                    }

                    sleep(HEARTBEAT_INTERVAL).await;
                    yield Event::keep_alive_with_detail("reading the batch result");
                }
            }
        };

        for event in synthesize(into_response(&id, outcome)?) {
            for mapped in super::map_event(event, is_structured) {
                yield mapped?;
            }
        }
    }))
}

/// Unwrap the single result of a finished batch into the message it produced.
///
/// A request the batch rejected is reported in the same shape a synchronous
/// request would have used, so the retry and thinking-repair layers classify it
/// identically whichever route it took.
fn into_response(id: &str, outcome: Outcome) -> Result<types::CreateMessagesResponse, StreamError> {
    match outcome {
        Outcome::Message(response) => Ok(*response),
        Outcome::Failed(error) => Err(StreamError::from(AnthropicError::Api(error))),
        Outcome::Canceled => Err(StreamError::other(format!(
            "Anthropic batch {id} was cancelled before the request reached the model."
        ))),
        Outcome::Expired => Err(StreamError::other(format!(
            "Anthropic batch {id} expired before the request reached the model."
        ))),
    }
}

/// Replay a finished message as the stream events its streaming counterpart
/// would have produced.
///
/// Everything downstream — chaining on the token ceiling, the forced-tool
/// fallback, the event builder — is written against the streaming shape.
/// Rebuilding that shape here keeps [`super::map_event`] the single place that
/// knows how Anthropic content maps onto JP events.
fn synthesize(response: types::CreateMessagesResponse) -> Vec<types::MessagesStreamEvent> {
    let mut events = vec![];

    for (index, block) in response.content.into_iter().enumerate() {
        // A tool call's arguments only reach `map_event` through a delta: the
        // block that opens the call carries its id and name and nothing else.
        let arguments = match &block {
            types::MessageContent::ToolUse(tool_use) => Some(tool_use.input.to_string()),
            _ => None,
        };

        events.push(types::MessagesStreamEvent::ContentBlockStart {
            index,
            content_block: block,
        });

        if let Some(partial_json) = arguments {
            events.push(types::MessagesStreamEvent::ContentBlockDelta {
                index,
                delta: types::ContentBlockDelta::InputJsonDelta { partial_json },
            });
        }

        events.push(types::MessagesStreamEvent::ContentBlockStop { index });
    }

    events.push(types::MessagesStreamEvent::MessageDelta {
        delta: types::MessageDelta {
            stop_reason: response.stop_reason,
            stop_sequence: response.stop_sequence,
            // A batch result reports the stop reason alone; the refusal
            // category and explanation ride only on the streaming event.
            stop_details: None,
        },
        usage: response.usage,
    });
    events.push(types::MessagesStreamEvent::MessageStop);

    events
}

/// Cancels the batch it holds when dropped.
///
/// Dropping the stream is how a caller abandons a turn, and a batch nobody is
/// waiting for keeps running and billing on Anthropic's side.
/// `Drop` cannot await, so the cancel goes out on a detached task and its
/// outcome is only logged.
struct CancelOnDrop {
    client: Client,
    id: Option<String>,
}

impl CancelOnDrop {
    /// Stop cancelling on drop, for a batch that has already ended.
    fn disarm(&mut self) {
        self.id = None;
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        let Some(id) = self.id.take() else {
            return;
        };

        // A drop during runtime shutdown has no runtime to spawn onto, and
        // `tokio::spawn` panics there rather than returning an error.
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            warn!(batch = %id, "Abandoned Anthropic batch left running: no runtime to cancel it on.");
            return;
        };

        let client = self.client.clone();
        handle.spawn(async move {
            match cancel(&client, &id).await {
                Ok(()) => info!(batch = %id, "Cancelled abandoned Anthropic batch."),
                Err(error) => {
                    warn!(batch = %id, %error, "Failed to cancel abandoned Anthropic batch.");
                }
            }
        });
    }
}

/// A Message Batch, reduced to the fields the polling loop reads.
#[derive(Debug, Deserialize)]
struct Batch {
    id: String,
    processing_status: ProcessingStatus,
}

/// How far along a batch is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ProcessingStatus {
    /// Requests are still being processed.
    InProgress,

    /// Cancellation was requested and is being finalized.
    Canceling,

    /// Every request has finished and the results are ready.
    Ended,

    /// A status this build does not know, treated as still running.
    #[serde(other)]
    Unknown,
}

/// The wire body for creating a batch.
#[derive(Debug, Serialize)]
struct CreateBatch {
    requests: [BatchRequest; 1],
}

/// One request within a batch.
///
/// `params` is a serialized [`types::CreateMessagesRequest`] rather than the
/// struct itself: the batch API rejects `stream`, which the struct always
/// serializes.
#[derive(Debug, Serialize)]
struct BatchRequest {
    custom_id: &'static str,
    params: Value,
}

/// What the model produced for the single request in a batch.
enum Outcome {
    /// The request was answered.
    Message(Box<types::CreateMessagesResponse>),

    /// The request failed, in the shape a synchronous request would report.
    Failed(ApiError),

    /// The batch was cancelled before the request reached the model.
    Canceled,

    /// The batch expired before the request reached the model.
    Expired,
}

/// One line of the batch results file.
#[derive(Debug, Deserialize)]
struct ResultLine {
    result: BatchResult,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum BatchResult {
    Succeeded {
        message: types::CreateMessagesResponse,
    },
    Errored {
        error: ErrorEnvelope,
    },
    Canceled,
    Expired,
}

/// Anthropic's error envelope, whose outer `type` is always `"error"`.
#[derive(Debug, Deserialize)]
struct ErrorEnvelope {
    error: ApiError,
}

/// Submit `request` as the only entry in a new batch.
///
/// Anthropic validates `params` asynchronously, so a malformed request is
/// accepted here and reported as an errored result once the batch ends.
async fn create(
    client: &Client,
    request: &types::CreateMessagesRequest,
    betas: &[String],
) -> Result<Batch, AnthropicError> {
    let body = CreateBatch {
        requests: [BatchRequest {
            custom_id: CUSTOM_ID,
            params: batch_params(request)?,
        }],
    };

    client.post(BATCHES_PATH, body, betas).await
}

/// Serialize `request` into the `params` object of a batch entry.
///
/// `stream` is dropped: the batch API rejects the key, and the request struct
/// always serializes it.
fn batch_params(request: &types::CreateMessagesRequest) -> Result<Value, AnthropicError> {
    let mut params = serde_json::to_value(request).map_err(AnthropicError::Deserialization)?;
    if let Some(object) = params.as_object_mut() {
        object.remove("stream");
    }

    Ok(params)
}

/// Read a batch's current processing status.
async fn retrieve(client: &Client, id: &str) -> Result<Batch, AnthropicError> {
    client.get(&format!("{BATCHES_PATH}/{id}")).await
}

/// Read the result of the single request in an ended batch.
///
/// The results endpoint serves JSONL.
/// A batch of one produces a single line, which is itself a complete JSON
/// document, so it deserializes directly.
///
/// The path is rebuilt from the batch id rather than taken from the batch's
/// `results_url`, which is absolute and would bypass a configured `base_url`.
async fn result(client: &Client, id: &str) -> Result<Outcome, AnthropicError> {
    let line: ResultLine = client.get(&format!("{BATCHES_PATH}/{id}/results")).await?;

    Ok(match line.result {
        BatchResult::Succeeded { message } => Outcome::Message(Box::new(message)),
        BatchResult::Errored { error } => Outcome::Failed(error.error),
        BatchResult::Canceled => Outcome::Canceled,
        BatchResult::Expired => Outcome::Expired,
    })
}

/// Ask Anthropic to stop processing a batch.
///
/// Requests the model has already answered are still billed and still readable;
/// the ones it has not reached are dropped.
async fn cancel(client: &Client, id: &str) -> Result<(), AnthropicError> {
    client
        .post::<_, Value>(
            &format!("{BATCHES_PATH}/{id}/cancel"),
            Map::<String, Value>::new(),
            &[],
        )
        .await
        .map(drop)
}

#[cfg(test)]
#[path = "batch_tests.rs"]
mod tests;
