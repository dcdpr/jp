use eventsource_stream::Event as MessageEvent;
use futures::StreamExt as _;
use jp_conversation::{ConversationEvent, event::ToolCallRequest};
use reqwest_eventsource::Error as SseError;

use super::*;
use crate::provider::openai_compat::StreamChunk;

/// Regression: a model absent from the table must still request the parsed
/// reasoning format.
/// Without it, Cerebras returns reasoning inline in `content` wrapped in
/// `<think>` tags, which leaks into the visible message instead of being
/// recorded as reasoning.
#[test]
fn test_unknown_model_requests_parsed_reasoning() {
    let model = ModelDetails::empty((PROVIDER, "future-model-99").try_into().unwrap());
    assert_eq!(model.reasoning, None, "fixture must be unknown");

    let query = ChatQuery {
        thread: jp_conversation::thread::Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events: jp_conversation::ConversationStream::new_test().with_turn("test"),
        },
        tools: vec![],
        tool_choice: ToolChoice::Auto,
    };

    let (body, _) = create_request(&model, query).unwrap();

    assert_eq!(
        body["reasoning_format"], "parsed",
        "unknown models must request parsed reasoning"
    );
}

/// Build a query whose only config is an explicit reasoning setting.
fn reasoning_query(reasoning: jp_config::model::parameters::PartialReasoningConfig) -> ChatQuery {
    let mut events = jp_conversation::ConversationStream::new_test().with_turn("test");
    let mut delta = jp_config::PartialAppConfig::empty();
    delta.assistant.model.parameters.reasoning = Some(reasoning);
    events.add_config_delta(delta);

    ChatQuery {
        thread: jp_conversation::thread::Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events,
        },
        tools: vec![],
        tool_choice: ToolChoice::Auto,
    }
}

/// An explicit `off` is honoured for a model whose reasoning support is
/// unknown.
/// Discarding it would silently ignore the caller, and a model absent from the
/// table may well accept `none`.
#[test]
fn create_request_honours_off_for_unknown_model() {
    let model = ModelDetails::empty((PROVIDER, "future-model-99").try_into().unwrap());
    assert_eq!(model.reasoning, None, "fixture must be unknown");

    let query = reasoning_query(jp_config::model::parameters::PartialReasoningConfig::Off);
    let (body, _) = create_request(&model, query).unwrap();

    assert_eq!(body["reasoning_effort"], "none");
}

/// `auto` on a model with unknown support omits the effort so the server picks
/// its own, while still requesting the parsed reasoning format so reasoning is
/// captured rather than inlined as `<think>` tags.
#[test]
fn create_request_auto_defers_effort_for_unknown_model() {
    let model = ModelDetails::empty((PROVIDER, "future-model-99").try_into().unwrap());

    let query = reasoning_query(jp_config::model::parameters::PartialReasoningConfig::Auto);
    let (body, _) = create_request(&model, query).unwrap();

    assert!(body.get("reasoning_effort").is_none());
    assert_eq!(body["reasoning_format"], "parsed");
}

/// A known ladder still resolves `auto` to a supported level, since there is
/// one to pick from.
#[test]
fn create_request_auto_uses_known_ladder() {
    let model = map_model("gpt-oss-120b").unwrap();

    let query = reasoning_query(jp_config::model::parameters::PartialReasoningConfig::Auto);
    let (body, _) = create_request(&model, query).unwrap();

    assert_eq!(body["reasoning_effort"], "medium");
}

/// `gemma-4-31b` reasons but reports no ladder, so support is unknown and an
/// explicit `off` is honoured via the unknown path.
/// The recorded fixture for this model confirms the provider accepts it.
#[test]
fn create_request_honours_off_for_gemma() {
    let model = map_model("gemma-4-31b").unwrap();
    assert_eq!(
        model.reasoning, None,
        "no ladder is reported for this model"
    );

    let query = reasoning_query(jp_config::model::parameters::PartialReasoningConfig::Off);
    let (body, _) = create_request(&model, query).unwrap();

    assert_eq!(body["reasoning_effort"], "none");
}

/// A model known not to accept a `none` effort omits the field entirely and
/// takes the server default, rather than sending a value it would reject.
#[test]
fn create_request_omits_off_for_model_without_none() {
    let model = map_model("gpt-oss-120b").unwrap();

    let query = reasoning_query(jp_config::model::parameters::PartialReasoningConfig::Off);
    let (body, _) = create_request(&model, query).unwrap();

    assert!(body.get("reasoning_effort").is_none());
}

/// Records a live request for a Cerebras model absent from the built-in table
/// with `auto` reasoning.
///
/// Validates that omitting `reasoning_effort` is accepted while still asking
/// for the parsed reasoning format, which is the shape the unit tests can only
/// assert about in the abstract.
#[tokio::test]
async fn test_unknown_model_auto_omits_effort() -> jp_test::Result {
    let id: ModelIdConfig = "cerebras/gemma-4-31b".parse().unwrap();

    // No table entry, so support is unknown and `auto` defers the effort.
    let details = ModelDetails::empty(id.clone());

    let request = crate::test::TestRequest::chat(PROVIDER)
        .model(id)
        .model_details(details)
        .reasoning(Some(
            jp_config::model::parameters::PartialReasoningConfig::Auto,
        ))
        .chat_request("What is 2 + 2?");

    crate::test::run_test(PROVIDER, jp_test::function_name!(), Some(request)).await
}

/// Records a live request for a model absent from the table with reasoning
/// explicitly off.
///
/// Validates that `reasoning_effort: "none"` is accepted, which JP now sends
/// for unknown models rather than discarding the caller's setting.
#[tokio::test]
async fn test_unknown_model_off_sends_none() -> jp_test::Result {
    let id: ModelIdConfig = "cerebras/gemma-4-31b".parse().unwrap();
    let details = ModelDetails::empty(id.clone());

    let request = crate::test::TestRequest::chat(PROVIDER)
        .model(id)
        .model_details(details)
        .reasoning(Some(
            jp_config::model::parameters::PartialReasoningConfig::Off,
        ))
        .chat_request("What is 2 + 2?");

    crate::test::run_test(PROVIDER, jp_test::function_name!(), Some(request)).await
}

fn sse_message(data: &str) -> SseEvent {
    SseEvent::Message(MessageEvent {
        data: data.to_owned(),
        ..MessageEvent::default()
    })
}

fn flush_indices(events: &[std::result::Result<Event, StreamError>]) -> Vec<usize> {
    events
        .iter()
        .filter_map(|e| match e {
            Ok(Event::Flush { index, .. }) => Some(*index),
            _ => None,
        })
        .collect()
}

#[test_log::test(tokio::test)]
async fn surfaces_stream_error_before_completion() {
    // A transport error before `[DONE]` (a dropped or stalled connection) must
    // surface as a `StreamError` so the retry layer can act on it, rather than
    // being silently swallowed.
    let content = sse_message(
        r#"{"choices":[{"delta":{"content":"partial"},"index":0,"finish_reason":null}]}"#,
    );
    let events = stream::iter(vec![Ok(content), Err(SseError::StreamEnded)]);

    let out: Vec<_> = assemble_event_stream(events, false).collect().await;

    assert!(
        out.iter().any(std::result::Result::is_err),
        "pre-completion stream error must surface, got {out:?}",
    );
}

#[test_log::test(tokio::test)]
async fn swallows_stream_error_after_completion() {
    // The connection close that follows `[DONE]` is the benign EOF; once the
    // stream has emitted `Finished` it must not be surfaced as an error.
    let content =
        sse_message(r#"{"choices":[{"delta":{"content":"hi"},"index":0,"finish_reason":"stop"}]}"#);
    let events = stream::iter(vec![
        Ok(content),
        Ok(sse_message("[DONE]")),
        Err(SseError::StreamEnded),
    ]);

    let out: Vec<_> = assemble_event_stream(events, false).collect().await;

    assert!(
        out.iter().all(std::result::Result::is_ok),
        "post-completion close must not surface an error, got {out:?}",
    );
    assert!(
        matches!(out.last(), Some(Ok(Event::Finished(_)))),
        "stream must end with Finished, got {:?}",
        out.last(),
    );
}

/// A rate limit is where a Cerebras user most needs to know that the reserved
/// quota per request is theirs to lower.
///
/// Cerebras reserves `min(max_completion_tokens, 16384)` tokens of the
/// per-minute bucket at admission, so a default request reserves 16384 however
/// few tokens it goes on to use.
/// The message is only read once retries are exhausted, which is exactly when
/// the setting is worth changing.
#[test_log::test(tokio::test)]
async fn a_rate_limit_names_the_setting_that_lowers_the_reservation() {
    let response = http::Response::builder()
        .status(429)
        .header("retry-after", "60")
        .body(
            r#"{"message":"Tokens per minute limit exceeded - too many tokens processed.","type":"too_many_tokens_error","param":"quota","code":"token_quota_exceeded"}"#
                .to_owned(),
        )
        .expect("valid response");
    let rate_limited = SseError::InvalidStatusCode(
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        reqwest::Response::from(response),
    );

    let out: Vec<_> = assemble_event_stream(stream::iter(vec![Err(rate_limited)]), false)
        .collect()
        .await;

    let [Err(error)] = out.as_slice() else {
        panic!("expected a single error, got {out:?}")
    };

    assert_eq!(error.kind, crate::error::StreamErrorKind::RateLimit);
    assert_eq!(error.retry_after, Some(Duration::from_mins(1)));
    assert!(
        error.message().contains("max_tokens"),
        "a rate limit must point at the setting, got: {}",
        error.message()
    );
}

/// Only a rate limit gets the hint; other failures are not about reservation
/// size and the advice would be misleading.
#[test_log::test(tokio::test)]
async fn other_failures_do_not_mention_the_reservation() {
    let response = http::Response::builder()
        .status(404)
        .body(
            r#"{"message":"Model zai-glm-4.7 is archived and unavailable for the organization.","type":"model_archived_error"}"#
                .to_owned(),
        )
        .expect("valid response");
    let not_found = SseError::InvalidStatusCode(
        reqwest::StatusCode::NOT_FOUND,
        reqwest::Response::from(response),
    );

    let out: Vec<_> = assemble_event_stream(stream::iter(vec![Err(not_found)]), false)
        .collect()
        .await;

    let [Err(error)] = out.as_slice() else {
        panic!("expected a single error, got {out:?}")
    };

    assert_eq!(
        error.message(),
        "Model zai-glm-4.7 is archived and unavailable for the organization. (HTTP 404 Not Found)"
    );
}

#[test]
fn parse_cerebras_content_chunk() {
    let json = r#"{
        "choices": [{
            "delta": { "content": "Hello!" },
            "index": 0,
            "finish_reason": null
        }]
    }"#;

    let chunk: StreamChunk = serde_json::from_str(json).unwrap();
    assert_eq!(chunk.choices[0].delta.content.as_deref(), Some("Hello!"));
    assert!(chunk.choices[0].finish_reason.is_none());
}

#[test]
fn parse_cerebras_reasoning_field() {
    // Cerebras parsed format returns reasoning in the `reasoning` field, which
    // is deserialized into `reasoning_content` via serde alias.
    let json = r#"{
        "choices": [{
            "delta": {
                "reasoning": "The user just says hello.",
                "content": null
            },
            "index": 0,
            "finish_reason": null
        }]
    }"#;

    let chunk: StreamChunk = serde_json::from_str(json).unwrap();
    let delta = &chunk.choices[0].delta;
    assert_eq!(
        delta.reasoning_content.as_deref(),
        Some("The user just says hello.")
    );
    assert!(delta.content.is_none());
}

#[test]
fn parse_cerebras_reasoning_content_field() {
    // The `reasoning_content` field name also works (DeepSeek-compatible).
    let json = r#"{
        "choices": [{
            "delta": {
                "reasoning_content": "step by step",
                "content": null
            },
            "index": 0,
            "finish_reason": null
        }]
    }"#;

    let chunk: StreamChunk = serde_json::from_str(json).unwrap();
    let delta = &chunk.choices[0].delta;
    assert_eq!(delta.reasoning_content.as_deref(), Some("step by step"));
}

#[test]
fn parse_cerebras_finish_reason_stop() {
    let json = r#"{
        "choices": [{
            "delta": {},
            "index": 0,
            "finish_reason": "stop"
        }]
    }"#;

    let chunk: StreamChunk = serde_json::from_str(json).unwrap();
    assert_eq!(chunk.choices[0].finish_reason.as_deref(), Some("stop"));
}

#[test]
fn parse_cerebras_tool_call() {
    let json = r#"{
        "choices": [{
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_xyz",
                    "function": {
                        "name": "read_file",
                        "arguments": "{\"path\":\"foo.rs\"}"
                    }
                }]
            },
            "index": 0,
            "finish_reason": null
        }]
    }"#;

    let chunk: StreamChunk = serde_json::from_str(json).unwrap();
    let tc = &chunk.choices[0].delta.tool_calls.as_ref().unwrap()[0];
    assert_eq!(tc.id.as_deref(), Some("call_xyz"));
    assert_eq!(
        tc.function.as_ref().unwrap().name.as_deref(),
        Some("read_file")
    );
}

#[test]
fn convert_tool_choice_values() {
    assert_eq!(convert_tool_choice(&ToolChoice::Auto), json!("auto"));
    assert_eq!(convert_tool_choice(&ToolChoice::None), json!("none"));
    assert_eq!(
        convert_tool_choice(&ToolChoice::Required),
        json!("required")
    );
    assert_eq!(
        convert_tool_choice(&ToolChoice::Function("my_fn".into())),
        json!({"type": "function", "function": {"name": "my_fn"}})
    );
}

/// The public catalog supplies limits, structured output, deprecation, and
/// whether the model reasons.
/// The effort ladder is not reported, so it keeps coming from the built-in
/// table.
#[test]
fn map_model_with_catalog_prefers_reported_limits() {
    let public: PublicModel = serde_json::from_value(json!({
        "id": "gpt-oss-120b",
        "name": "OpenAI GPT OSS",
        "limits": {"max_context_length": 262_144, "max_completion_tokens": 65_536},
        "capabilities": {"reasoning": true, "structured_outputs": true},
        "deprecated": false,
    }))
    .unwrap();

    let details = map_model_with_catalog("gpt-oss-120b", Some(&public)).unwrap();

    assert_eq!(details.display_name.as_deref(), Some("OpenAI GPT OSS"));
    assert_eq!(details.context_window, Some(262_144));
    assert_eq!(details.max_output_tokens, Some(65_536));
    assert_eq!(details.structured_output, Some(true));
    assert_eq!(details.deprecated, Some(ModelDeprecation::Active));
    // Ladder retained from the table, which the catalog cannot express.
    assert!(details.reasoning.unwrap().is_leveled());
}

/// A catalog entry reporting no reasoning overrides a table claiming otherwise.
#[test]
fn map_model_with_catalog_corrects_reasoning_support() {
    let public: PublicModel = serde_json::from_value(json!({
        "id": "gpt-oss-120b",
        "capabilities": {"reasoning": false},
    }))
    .unwrap();

    let details = map_model_with_catalog("gpt-oss-120b", Some(&public)).unwrap();

    assert_eq!(details.reasoning, Some(ReasoningDetails::unsupported()));
}

/// A model the catalog reports as reasoning but the table has no ladder for is
/// left unknown rather than given invented effort levels.
#[test]
fn map_model_with_catalog_unknown_ladder_stays_unknown() {
    let public: PublicModel = serde_json::from_value(json!({
        "id": "some-future-model",
        "capabilities": {"reasoning": true},
    }))
    .unwrap();

    let details = map_model_with_catalog("some-future-model", Some(&public)).unwrap();

    assert_eq!(details.reasoning, None);
}

/// Without a catalog entry the built-in table is used unchanged, which is the
/// path taken whenever the unauthenticated catalog is unreachable.
#[test]
fn map_model_with_catalog_falls_back_to_table() {
    let details = map_model_with_catalog("gpt-oss-120b", None).unwrap();

    assert_eq!(details.context_window, Some(131_072));
    assert_eq!(details.max_output_tokens, Some(40_960));
    assert!(details.reasoning.unwrap().is_leveled());
}

/// A catalog entry reporting no capabilities leaves the table's values intact,
/// rather than reading the absent fields as "unsupported".
#[test]
fn map_model_with_catalog_absent_capabilities_keeps_table() {
    let public: PublicModel = serde_json::from_value(json!({
        "id": "gpt-oss-120b",
        "limits": {"max_context_length": 262_144},
    }))
    .unwrap();

    let details = map_model_with_catalog("gpt-oss-120b", Some(&public)).unwrap();

    // Reported, so applied.
    assert_eq!(details.context_window, Some(262_144));
    // Unreported, so the table stands.
    assert_eq!(details.max_output_tokens, Some(40_960));
    assert_eq!(details.structured_output, Some(true));
    assert!(details.reasoning.unwrap().is_leveled());
}

#[test]
fn map_model_known() {
    let details = map_model("gemma-4-31b").unwrap();
    assert_eq!(details.display_name.as_deref(), Some("Gemma 4 31B"));
    assert_eq!(details.context_window, Some(131_072));
    assert_eq!(details.max_output_tokens, Some(40_960));
    // Limits are known, but the catalog names no effort levels, so the ladder
    // stays unknown rather than being invented.
    assert_eq!(details.reasoning, None);
}

#[test]
fn map_model_gpt_oss_has_reasoning() {
    let details = map_model("gpt-oss-120b").unwrap();
    assert!(details.reasoning.unwrap().is_leveled());
}

#[test]
fn map_model_unknown_returns_empty() {
    let details = map_model("some-future-model").unwrap();
    assert!(details.display_name.is_none());
    assert!(details.context_window.is_none());
}

#[test]
fn transform_schema_moves_array_constraints_to_description() {
    let schema: serde_json::Map<String, Value> = serde_json::from_value(json!({
        "type": "object",
        "properties": {
            "tags": {
                "type": "array",
                "items": { "type": "string" },
                "minItems": 1,
                "maxItems": 5
            }
        }
    }))
    .unwrap();

    let result = transform_schema(schema);
    let tags = &result["properties"]["tags"];

    // Unsupported fields removed from schema.
    assert!(tags.get("minItems").is_none());
    assert!(tags.get("maxItems").is_none());

    // But preserved as a description hint.
    let desc = tags["description"].as_str().unwrap();
    assert!(desc.contains("minItems"), "desc = {desc}");
    assert!(desc.contains("maxItems"), "desc = {desc}");

    // Supported fields still present.
    assert_eq!(tags["type"], "array");
    assert_eq!(tags["items"]["type"], "string");
}

#[test]
fn transform_schema_moves_string_constraints_to_description() {
    let schema: serde_json::Map<String, Value> = serde_json::from_value(json!({
        "type": "object",
        "properties": {
            "email": {
                "type": "string",
                "description": "An email address",
                "format": "email",
                "pattern": "^.+@.+$"
            }
        }
    }))
    .unwrap();

    let result = transform_schema(schema);
    let email = &result["properties"]["email"];

    assert!(email.get("format").is_none());
    assert!(email.get("pattern").is_none());

    // Hints appended to existing description.
    let desc = email["description"].as_str().unwrap();
    assert!(desc.starts_with("An email address"), "desc = {desc}");
    assert!(desc.contains("format"), "desc = {desc}");
    assert!(desc.contains("pattern"), "desc = {desc}");
}

#[test]
fn transform_schema_forces_strict_objects() {
    let schema: serde_json::Map<String, Value> = serde_json::from_value(json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "nested": {
                "type": "object",
                "properties": {
                    "value": { "type": "integer" }
                }
            }
        },
        "required": ["name"]
    }))
    .unwrap();

    let result = transform_schema(schema);

    // Root object: additionalProperties false, all props required.
    assert_eq!(result["additionalProperties"], false);
    let required = result["required"].as_array().unwrap();
    assert!(required.contains(&json!("name")));
    assert!(required.contains(&json!("nested")));

    // Nested object: same treatment.
    let nested = &result["properties"]["nested"];
    assert_eq!(nested["additionalProperties"], false);
    let nested_req = nested["required"].as_array().unwrap();
    assert!(nested_req.contains(&json!("value")));
}

#[test]
fn transform_schema_preserves_number_constraints() {
    let schema: serde_json::Map<String, Value> = serde_json::from_value(json!({
        "type": "object",
        "properties": {
            "age": {
                "type": "integer",
                "minimum": 0,
                "maximum": 150
            }
        }
    }))
    .unwrap();

    let result = transform_schema(schema);
    let age = &result["properties"]["age"];
    assert_eq!(age["minimum"], 0);
    assert_eq!(age["maximum"], 150);
    assert!(age.get("description").is_none());
}

/// `finish_reason: "length"` followed by `[DONE]` must not flush any pending
/// tool-call buffers.
/// When the model hits the token limit mid-tool-call, the arguments are
/// structurally incomplete; the safety-net drain on `[DONE]` would otherwise
/// commit them with truncated JSON (degraded to `{}`), which could re-dispatch
/// a partial call.
#[test]
fn length_finish_reason_drops_pending_tool_calls() {
    let mut state = StreamState {
        tool_call_indices: Vec::new(),
        reasoning_flushed: false,
        message_flushed: false,
        finished: false,
        finish_reason: None,
        is_structured: false,
    };

    // Tool call delta with partial arguments.
    let tool_chunk = r#"{
        "choices": [{
            "delta": {
                "tool_calls": [{
                    "index": 0,
                    "id": "call_xyz",
                    "function": { "name": "run_me", "arguments": "{\"path\":" }
                }]
            },
            "index": 0,
            "finish_reason": null
        }]
    }"#;
    handle_sse_event_sync(Ok(sse_message(tool_chunk)), &mut state).unwrap();
    assert_eq!(state.tool_call_indices, vec![2]);

    // Terminal `"length"` chunk: should clear the pending tool-call index so
    // the `[DONE]` safety net cannot commit the truncated buffer.
    let finish_chunk = r#"{
        "choices": [{
            "delta": {},
            "index": 0,
            "finish_reason": "length"
        }]
    }"#;
    let finish_events = handle_sse_event_sync(Ok(sse_message(finish_chunk)), &mut state).unwrap();
    // Reasoning was already flushed when the tool-call chunk arrived, so only
    // the message index flushes here. The tool-call index must NOT be in this
    // list — that's the bug guard.
    assert_eq!(
        flush_indices(&finish_events),
        vec![1],
        "only message index should flush on length, got {finish_events:?}"
    );
    assert!(
        state.tool_call_indices.is_empty(),
        "length must drop pending tool-call indices, got {:?}",
        state.tool_call_indices,
    );
    assert_eq!(state.finish_reason, Some(FinishReason::MaxTokens));

    // `[DONE]` safety net: must NOT flush the tool-call index, and must
    // finish with MaxTokens.
    let done_events = handle_sse_event_sync(Ok(sse_message("[DONE]")), &mut state).unwrap();
    assert!(
        flush_indices(&done_events).is_empty(),
        "[DONE] after length must not flush any indices, got {done_events:?}"
    );
    let last = done_events.last().unwrap().as_ref().unwrap();
    assert!(
        matches!(last, Event::Finished(FinishReason::MaxTokens)),
        "expected Finished(MaxTokens), got {last:?}"
    );
}

#[test]
fn convert_events_coalesces_parallel_tool_calls() {
    let mut stream = ConversationStream::new_test();
    stream.extend([
        ConversationEvent::from(ChatResponse::reasoning("Both files look relevant.")),
        ConversationEvent::from(ChatResponse::message("Let me read both files.")),
        ConversationEvent::from(ToolCallRequest {
            id: "call_a".into(),
            name: "fs_read_file".into(),
            arguments: Map::new(),
        }),
        ConversationEvent::from(ToolCallRequest {
            id: "call_b".into(),
            name: "fs_read_file".into(),
            arguments: Map::new(),
        }),
        ConversationEvent::from(ToolCallResponse {
            id: "call_a".into(),
            result: Ok("lib.rs".into()),
        }),
        ConversationEvent::from(ToolCallResponse {
            id: "call_b".into(),
            result: Ok("Cargo.toml".into()),
        }),
    ]);

    let messages = convert_events(stream);

    // One model turn -- reasoning, content, and both parallel tool calls --
    // collapses into a single assistant message, followed by one `tool`
    // message per result.
    assert_eq!(messages.len(), 3, "{messages:#?}");

    assert_eq!(messages[0]["role"], "assistant");
    assert_eq!(messages[0]["reasoning"], "Both files look relevant.");
    assert_eq!(messages[0]["content"], "Let me read both files.");
    let tool_calls = messages[0]["tool_calls"].as_array().unwrap();
    assert_eq!(tool_calls.len(), 2);
    assert_eq!(tool_calls[0]["id"], "call_a");
    assert_eq!(tool_calls[1]["id"], "call_b");

    assert_eq!(messages[1]["role"], "tool");
    assert_eq!(messages[1]["tool_call_id"], "call_a");
    assert_eq!(messages[2]["role"], "tool");
    assert_eq!(messages[2]["tool_call_id"], "call_b");
}
