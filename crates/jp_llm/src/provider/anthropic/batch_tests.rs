use std::time::{Duration, Instant};

use async_anthropic::types::{
    CreateMessagesRequest, CreateMessagesRequestBuilder, CreateMessagesResponse, MessageContent,
    MessagesStreamEvent, Text, Thinking, ToolUse, Usage,
};
use futures::StreamExt as _;
use jp_test::mock::{GET, MockServer, POST};
use serde_json::json;
use test_log::test;

use super::*;
use crate::{
    error::StreamErrorKind,
    event::{EventPart, FinishReason},
    provider::anthropic::map_event,
};

/// A response carrying `content`, with the fields a finished message always
/// has.
fn response(content: Vec<MessageContent>) -> CreateMessagesResponse {
    CreateMessagesResponse {
        id: Some("msg_0000000000000000000000".to_owned()),
        content,
        model: Some("claude-opus-5".to_owned()),
        stop_reason: Some("end_turn".to_owned()),
        stop_sequence: None,
        usage: Some(Usage {
            input_tokens: Some(11),
            output_tokens: Some(22),
        }),
    }
}

#[test]
fn batch_params_drop_the_stream_key() {
    let request = CreateMessagesRequestBuilder::default()
        .model("claude-opus-5")
        .messages(vec!["hello".into()])
        .max_tokens(64)
        .build()
        .unwrap();

    let params = batch_params(&request).unwrap();
    let object = params.as_object().unwrap();

    // The batch API rejects `stream` outright, and the request struct
    // serializes it whether it is set or not.
    assert!(
        !object.contains_key("stream"),
        "params must not carry `stream`, got {params}"
    );
    assert_eq!(object.get("model"), Some(&json!("claude-opus-5")));
    assert_eq!(object.get("max_tokens"), Some(&json!(64)));
}

#[test]
fn synthesize_replays_a_text_block_as_start_stop_and_message_end() {
    let events = synthesize(response(vec![MessageContent::Text(Text {
        text: "Hello.".to_owned(),
        cache_control: None,
    })]));

    assert_eq!(events, vec![
        MessagesStreamEvent::ContentBlockStart {
            index: 0,
            content_block: MessageContent::Text(Text {
                text: "Hello.".to_owned(),
                cache_control: None,
            }),
        },
        MessagesStreamEvent::ContentBlockStop { index: 0 },
        MessagesStreamEvent::MessageDelta {
            delta: types::MessageDelta {
                stop_reason: Some("end_turn".to_owned()),
                stop_sequence: None,
                stop_details: None,
            },
            usage: Some(Usage {
                input_tokens: Some(11),
                output_tokens: Some(22),
            }),
        },
        MessagesStreamEvent::MessageStop,
    ]);
}

/// A tool call's arguments reach `map_event` only through an input-json delta:
/// the block that opens the call carries its id and name and nothing else.
/// Without the synthetic delta the call would arrive with empty arguments.
#[test]
fn synthesize_replays_tool_call_arguments_as_a_delta() {
    let events = synthesize(response(vec![MessageContent::ToolUse(ToolUse {
        id: "toolu_0000000000000000000000".to_owned(),
        name: "read_file".to_owned(),
        input: json!({ "path": "src/lib.rs" }),
        cache_control: None,
    })]));

    assert_eq!(
        events.get(1),
        Some(&MessagesStreamEvent::ContentBlockDelta {
            index: 0,
            delta: types::ContentBlockDelta::InputJsonDelta {
                partial_json: r#"{"path":"src/lib.rs"}"#.to_owned(),
            },
        })
    );
}

/// Each content block gets its own index, so the event builder keeps reasoning,
/// prose, and tool calls apart the way the streaming endpoint does.
#[test]
fn synthesize_indexes_each_content_block() {
    let events = synthesize(response(vec![
        MessageContent::Thinking(Thinking {
            thinking: "Considering.".to_owned(),
            signature: Some("sig".to_owned()),
        }),
        MessageContent::Text(Text {
            text: "Answer.".to_owned(),
            cache_control: None,
        }),
    ]));

    let indices: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            MessagesStreamEvent::ContentBlockStart { index, .. } => Some(*index),
            _ => None,
        })
        .collect();

    assert_eq!(indices, vec![0, 1]);
}

/// The point of replaying the streaming shape: a batched tool call produces the
/// same JP events a streamed one does, so everything downstream is unaware of
/// which route the request took.
#[test]
fn a_synthesized_tool_call_maps_to_the_same_jp_events_as_a_streamed_one() {
    let events: Vec<_> = synthesize(response(vec![MessageContent::ToolUse(ToolUse {
        id: "toolu_0000000000000000000000".to_owned(),
        name: "read_file".to_owned(),
        input: json!({ "path": "src/lib.rs" }),
        cache_control: None,
    })]))
    .into_iter()
    .flat_map(|event| map_event(event, false))
    .map(|event| event.expect("mapping a synthesized event cannot fail"))
    .collect();

    assert_eq!(events, vec![
        Event::tool_call_start(0, "toolu_0000000000000000000000", "read_file"),
        Event::tool_call_args(0, r#"{"path":"src/lib.rs"}"#),
        Event::flush(0),
        Event::Finished(FinishReason::Completed),
    ]);
}

/// A response stopped by the token ceiling has to surface as `MaxTokens`, since
/// that is what drives the chaining path.
#[test]
fn a_max_tokens_stop_reason_survives_the_replay() {
    let mut response = response(vec![MessageContent::Text(Text {
        text: "Half an ans".to_owned(),
        cache_control: None,
    })]);
    response.stop_reason = Some("max_tokens".to_owned());

    let events: Vec<_> = synthesize(response)
        .into_iter()
        .flat_map(|event| map_event(event, false))
        .map(|event| event.expect("mapping a synthesized event cannot fail"))
        .collect();

    assert!(
        events.contains(&Event::Finished(FinishReason::MaxTokens)),
        "expected a MaxTokens finish, got {events:?}"
    );
}

/// Structured output arrives as a text block, and has to be replayed as a
/// structured part rather than assistant prose.
#[test]
fn a_structured_response_replays_as_structured_content() {
    let events: Vec<_> = synthesize(response(vec![MessageContent::Text(Text {
        text: r#"{"answer":42}"#.to_owned(),
        cache_control: None,
    })]))
    .into_iter()
    .flat_map(|event| map_event(event, true))
    .map(|event| event.expect("mapping a synthesized event cannot fail"))
    .collect();

    assert_eq!(
        events.first(),
        Some(&Event::Part {
            index: 0,
            part: EventPart::Structured(r#"{"answer":42}"#.to_owned()),
            metadata: serde_json::Map::new(),
        })
    );
}

#[test]
fn a_succeeded_result_line_carries_the_message() {
    let line: ResultLine = serde_json::from_str(
        r#"{"custom_id":"jp","result":{"type":"succeeded","message":{"id":"msg_01","type":"message","role":"assistant","model":"claude-opus-5","content":[{"type":"text","text":"Hi."}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":10,"output_tokens":2}}}}"#,
    )
    .unwrap();

    let BatchResult::Succeeded { message } = line.result else {
        panic!("expected a succeeded result");
    };

    assert_eq!(message.stop_reason, Some("end_turn".to_owned()));
    assert_eq!(
        message.content.first().and_then(MessageContent::as_text),
        Some(&Text {
            text: "Hi.".to_owned(),
            cache_control: None,
        })
    );
}

/// A batch validates `params` asynchronously, so a malformed request comes back
/// as an errored result rather than a rejected submission.
/// It has to unwrap to the same `ApiError` a synchronous call would report, or
/// the retry and thinking-repair layers classify it differently depending on
/// the route.
#[test]
fn an_errored_result_line_unwraps_to_the_api_error() {
    let line: ResultLine = serde_json::from_str(
        r#"{"custom_id":"jp","result":{"type":"errored","error":{"type":"error","error":{"type":"invalid_request_error","message":"thinking blocks were modified"}}}}"#,
    )
    .unwrap();

    let BatchResult::Errored { error } = line.result else {
        panic!("expected an errored result");
    };

    assert_eq!(error.error.error_type, "invalid_request_error");
    assert_eq!(
        error.error.message.as_deref(),
        Some("thinking blocks were modified")
    );
}

#[test]
fn terminal_result_types_deserialize() {
    let canceled: ResultLine =
        serde_json::from_str(r#"{"custom_id":"jp","result":{"type":"canceled"}}"#).unwrap();
    assert!(matches!(canceled.result, BatchResult::Canceled));

    let expired: ResultLine =
        serde_json::from_str(r#"{"custom_id":"jp","result":{"type":"expired"}}"#).unwrap();
    assert!(matches!(expired.result, BatchResult::Expired));
}

/// A status Anthropic adds later must not read as "finished": treating an
/// unknown status as ended would fetch results that do not exist yet.
#[test]
fn an_unknown_processing_status_is_not_ended() {
    let batch: Batch = serde_json::from_str(
        r#"{"id":"msgbatch_01","processing_status":"something_new","request_counts":{}}"#,
    )
    .unwrap();

    assert_eq!(batch.id, "msgbatch_01");
    assert_eq!(batch.processing_status, ProcessingStatus::Unknown);
    assert_ne!(batch.processing_status, ProcessingStatus::Ended);
}

#[test]
fn known_processing_statuses_deserialize() {
    for (raw, expected) in [
        ("in_progress", ProcessingStatus::InProgress),
        ("canceling", ProcessingStatus::Canceling),
        ("ended", ProcessingStatus::Ended),
    ] {
        let batch: Batch =
            serde_json::from_str(&format!(r#"{{"id":"b","processing_status":"{raw}"}}"#)).unwrap();

        assert_eq!(batch.processing_status, expected, "for {raw}");
    }
}

/// A client pointed at `server`, standing in for the Anthropic API.
fn mock_client(server: &MockServer) -> Client {
    let mut builder = Client::builder();
    builder
        .api_key("test-key")
        .base_url(server.base_url())
        .version("2023-06-01");

    builder.build().expect("a client for the mock server")
}

/// A minimal request to batch.
fn request() -> CreateMessagesRequest {
    CreateMessagesRequestBuilder::default()
        .model("claude-test")
        .messages(vec!["go on then".into()])
        .max_tokens(16)
        .stream(true)
        .build()
        .expect("a valid request")
}

/// Poll one status check per second, and never give up.
///
/// One second is [`MIN_POLL_INTERVAL`], so a test that waits through a single
/// interval costs a second rather than the configured default of fifteen.
const FAST_POLL: PollConfig = PollConfig {
    interval: MIN_POLL_INTERVAL,
    max_wait: None,
};

/// A batch that is already finished when first polled still produces the same
/// JP events a streamed response would, and reports the wait while it lasts.
#[test(tokio::test)]
async fn a_finished_batch_replays_as_a_streamed_response() {
    let server = MockServer::start_async().await;

    // Matching on the body is what pins the submitted shape end to end: a mock
    // that does not match answers 404 and fails the test. `stream` matters
    // specifically — the batch API rejects the key outright.
    let create = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/v1/messages/batches")
                .body_includes(r#""custom_id":"jp""#)
                .body_includes(r#""model":"claude-test""#)
                .body_excludes(r#""stream""#);
            then.status(200).json_body(json!({
                "id": "msgbatch_test",
                "processing_status": "in_progress",
            }));
        })
        .await;

    let retrieve = server
        .mock_async(|when, then| {
            when.method(GET).path("/v1/messages/batches/msgbatch_test");
            then.status(200).json_body(json!({
                "id": "msgbatch_test",
                "processing_status": "ended",
            }));
        })
        .await;

    let results = server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1/messages/batches/msgbatch_test/results");
            then.status(200).body(
                r#"{"custom_id":"jp","result":{"type":"succeeded","message":{"id":"msg_01","type":"message","role":"assistant","model":"claude-test","content":[{"type":"text","text":"Answered."}],"stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":1,"output_tokens":1}}}}"#,
            );
        })
        .await;

    let events: Vec<_> = events(mock_client(&server), request(), false, FAST_POLL)
        .map(|event| event.expect("the batch route must not error"))
        .collect()
        .await;

    // The keep-alive is what stops the idle timeout from tearing the stream
    // down and submitting a second, separately billed batch.
    assert_eq!(events, vec![
        Event::keep_alive_with_detail("waiting on the batch"),
        Event::message(0, "Answered."),
        Event::flush(0),
        Event::Finished(FinishReason::Completed),
    ]);

    assert_eq!(create.calls_async().await, 1);
    assert_eq!(retrieve.calls_async().await, 1);
    assert_eq!(results.calls_async().await, 1);
}

/// A request the batch rejects has to reach the caller as the API error it is,
/// so the retry and thinking-repair layers classify it the same way they would
/// for a synchronous request.
#[test(tokio::test)]
async fn an_errored_result_surfaces_as_the_api_error() {
    let server = MockServer::start_async().await;

    server
        .mock_async(|when, then| {
            when.method(POST).path("/v1/messages/batches");
            then.status(200).json_body(json!({
                "id": "msgbatch_test",
                "processing_status": "ended",
            }));
        })
        .await;

    server
        .mock_async(|when, then| {
            when.method(GET).path("/v1/messages/batches/msgbatch_test");
            then.status(200).json_body(json!({
                "id": "msgbatch_test",
                "processing_status": "ended",
            }));
        })
        .await;

    server
        .mock_async(|when, then| {
            when.method(GET)
                .path("/v1/messages/batches/msgbatch_test/results");
            then.status(200).body(
                r#"{"custom_id":"jp","result":{"type":"errored","error":{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}}}"#,
            );
        })
        .await;

    let cancel = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/v1/messages/batches/msgbatch_test/cancel");
            then.status(200).json_body(json!({}));
        })
        .await;

    let events: Vec<_> = events(mock_client(&server), request(), false, FAST_POLL)
        .collect()
        .await;

    let [Err(error)] = events.as_slice() else {
        panic!("expected a single error, got {events:?}");
    };

    // `overloaded_error` is one Anthropic recommends retrying, and the
    // classification has to survive the trip through the batch result.
    assert_eq!(error.kind, StreamErrorKind::Transient);
    assert!(
        error.to_string().contains("Overloaded"),
        "the API message must reach the caller, got {error}"
    );

    // An ended batch has nothing left to cancel, and cancelling it would report
    // a failure the user cannot act on.
    assert_eq!(cancel.calls_async().await, 0);
}

/// Abandoning the stream is how the CLI ends a turn early.
/// The batch keeps running and billing on Anthropic's side unless something
/// stops it.
#[test(tokio::test)]
async fn dropping_the_stream_cancels_a_running_batch() {
    let server = MockServer::start_async().await;

    server
        .mock_async(|when, then| {
            when.method(POST).path("/v1/messages/batches");
            then.status(200).json_body(json!({
                "id": "msgbatch_test",
                "processing_status": "in_progress",
            }));
        })
        .await;

    // Never finishes, so the only way out of the stream is to drop it.
    server
        .mock_async(|when, then| {
            when.method(GET).path("/v1/messages/batches/msgbatch_test");
            then.status(200).json_body(json!({
                "id": "msgbatch_test",
                "processing_status": "in_progress",
            }));
        })
        .await;

    let cancel = server
        .mock_async(|when, then| {
            when.method(POST)
                .path("/v1/messages/batches/msgbatch_test/cancel");
            then.status(200).json_body(json!({}));
        })
        .await;

    let mut stream = events(mock_client(&server), request(), false, FAST_POLL);

    // Pull one item so the batch is submitted and the wait is under way. The
    // first item can only be a keep-alive: the batch never ends.
    assert_eq!(
        stream.next().await.transpose().unwrap(),
        Some(Event::keep_alive_with_detail("waiting on the batch"))
    );

    drop(stream);

    // The cancel goes out on a detached task, so there is nothing to await.
    // Two seconds is far longer than a request to a local mock server takes,
    // and a cancel that never arrives fails rather than passing on a lucky
    // poll.
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let calls = cancel.calls_async().await;
        if calls >= 1 {
            break;
        }

        assert!(
            Instant::now() < deadline,
            "the abandoned batch was never cancelled"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The batch body has to name each request, and the id has to satisfy
/// Anthropic's `^[a-zA-Z0-9_-]{1,64}$`.
#[test]
fn the_batch_body_wraps_one_named_request() {
    let request = CreateMessagesRequestBuilder::default()
        .model("claude-opus-5")
        .messages(vec!["hello".into()])
        .build()
        .unwrap();

    let body = CreateBatch {
        requests: [BatchRequest {
            custom_id: CUSTOM_ID,
            params: batch_params(&request).unwrap(),
        }],
    };

    let value = serde_json::to_value(&body).unwrap();
    let requests = value["requests"].as_array().unwrap();

    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["custom_id"], json!("jp"));
    assert_eq!(requests[0]["params"]["model"], json!("claude-opus-5"));
}
