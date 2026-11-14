use std::sync::Arc;

use jp_config::assistant::tool_choice::ToolChoice;
use jp_conversation::{
    event::{ChatRequest, ChatResponse, ToolCallRequest, ToolCallResponse},
    stream::ConversationEventWithConfig,
};
use jp_test::{Result, function_name};
use serde_json::{Map, Value, json};

use super::*;
use crate::{
    error::StreamErrorKind,
    test::{
        ProviderTestMode, TestEvent, TestRefusal, TestRequest, fixture_attachment, run_test_mode,
        test_model_details,
    },
};

/// Generate every provider's recorded suite, once per billing route.
///
/// `every_route` cases describe behavior a provider owes on each route it
/// sells, so they are generated for all of them.
/// A provider billed one way skips its subscription suite at runtime, and gains
/// the whole set the moment it implements
/// [`ProviderTestSupport::subscription`].
///
/// `api_only` cases ask a provider to enumerate its catalog, which is a
/// property of the metered API.
/// A subscription serves the fixed model set its plan includes and offers
/// nothing to enumerate.
///
/// [`ProviderTestSupport::subscription`]: crate::provider::ProviderTestSupport::subscription
macro_rules! test_all_providers {
    (
        every_route: [$($route_case:ident),* $(,)?],
        api_only: [$($api_case:ident),* $(,)?] $(,)?
    ) => {
        test_all_providers!(provider; anthropic, ProviderId::Anthropic, [$($route_case),*], [$($api_case),*]);
        test_all_providers!(provider; cerebras, ProviderId::Cerebras, [$($route_case),*], [$($api_case),*]);
        test_all_providers!(provider; google, ProviderId::Google, [$($route_case),*], [$($api_case),*]);
        test_all_providers!(provider; llamacpp, ProviderId::Llamacpp, [$($route_case),*], [$($api_case),*]);
        test_all_providers!(provider; ollama, ProviderId::Ollama, [$($route_case),*], [$($api_case),*]);
        test_all_providers!(provider; openai, ProviderId::Openai, [$($route_case),*], [$($api_case),*]);
        test_all_providers!(provider; openrouter, ProviderId::Openrouter, [$($route_case),*], [$($api_case),*]);
        test_all_providers!(provider; vllm, ProviderId::Vllm, [$($route_case),*], [$($api_case),*]);
    };
    (provider; $name:ident, $id:expr, [$($route_case:ident),*], [$($api_case:ident),*]) => {
        mod $name {
            use super::*;

            $(test_all_providers!(case; $route_case, $id, ProviderTestMode::Api);)*
            $(test_all_providers!(case; $api_case, $id, ProviderTestMode::Api);)*
        }

        paste::paste! {
            mod [< $name _subscription >] {
                use super::*;

                $(test_all_providers!(case; $route_case, $id, ProviderTestMode::Subscription);)*
            }
        }
    };
    (case; $case:ident, $id:expr, $mode:expr) => {
        paste::paste! {
            #[test_log::test(tokio::test)]
            async fn [< test_ $case >]() -> Result {
                $case($id, function_name!(), $mode).await
            }
        }
    };
}

async fn chat_completion_stream(
    provider: ProviderId,
    test_name: &str,
    mode: ProviderTestMode,
) -> Result {
    let request = TestRequest::chat(provider)
        .enable_reasoning()
        .event(ChatRequest::from("Test message"));

    run_test_mode(provider, test_name, Some(request), mode).await
}

fn tool_call_base(provider: ProviderId) -> TestRequest {
    TestRequest::chat(provider)
        .event(ChatRequest::from(
            "Please run the tool, providing whatever arguments you want.",
        ))
        .tool(
            "run_me",
            json!({
                "type": "object",
                "properties": {
                    "foo": { "type": "string", "default": "foo" },
                    "bar": {
                        "type": ["string", "array"],
                        "enum": ["foo"],
                        "items": { "type": "string", "enum": ["foo", "bar"] }
                    }
                },
                "required": ["bar"]
            }),
        )
}

async fn tool_call_stream(provider: ProviderId, test_name: &str, mode: ProviderTestMode) -> Result {
    let requests = vec![
        tool_call_base(provider),
        TestRequest::tool_call_response(Ok("working!"), false),
    ];

    run_test_mode(provider, test_name, requests, mode).await
}

/// Without reasoning, "forced" tool calls should work as expected.
async fn tool_call_required_no_reasoning(
    provider: ProviderId,
    test_name: &str,
    mode: ProviderTestMode,
) -> Result {
    let requests = vec![
        tool_call_base(provider).tool_choice(ToolChoice::Required),
        TestRequest::tool_call_response(Ok("working!"), true),
    ];

    run_test_mode(provider, test_name, requests, mode).await
}

/// With reasoning, some models do not support "forced" tool calls, so provider
/// implementations should fall back to trying to instruct the model to use the
/// tool through regular textual instructions.
async fn tool_call_required_reasoning(
    provider: ProviderId,
    test_name: &str,
    mode: ProviderTestMode,
) -> Result {
    let requests = vec![
        tool_call_base(provider)
            .tool_choice(ToolChoice::Required)
            .enable_reasoning(),
        TestRequest::tool_call_response(Ok("working!"), false),
    ];

    run_test_mode(provider, test_name, requests, mode).await
}

async fn tool_call_auto(provider: ProviderId, test_name: &str, mode: ProviderTestMode) -> Result {
    let requests = vec![
        tool_call_base(provider).tool_choice(ToolChoice::Auto),
        TestRequest::tool_call_response(Ok("working!"), false),
    ];

    run_test_mode(provider, test_name, requests, mode).await
}

async fn tool_call_function(
    provider: ProviderId,
    test_name: &str,
    mode: ProviderTestMode,
) -> Result {
    let requests = vec![
        tool_call_base(provider).tool_choice_fn("run_me"),
        TestRequest::tool_call_response(Ok("working!"), true),
    ];

    run_test_mode(provider, test_name, requests, mode).await
}

async fn tool_call_reasoning(
    provider: ProviderId,
    test_name: &str,
    mode: ProviderTestMode,
) -> Result {
    let requests = vec![
        tool_call_base(provider).enable_reasoning(),
        TestRequest::tool_call_response(Ok("working!"), false),
    ];

    run_test_mode(provider, test_name, requests, mode).await
}

/// Whether new cases are recorded for `provider` on `mode`.
///
/// Anthropic's subscription route records with a direct subscription token,
/// which is not an authorized use of the subscription, so no new recordings are
/// made against it.
fn records_new_cases(provider: ProviderId, mode: ProviderTestMode) -> bool {
    !(provider == ProviderId::Anthropic && mode == ProviderTestMode::Subscription)
}

fn tool_calls(history: &[ConversationEventWithConfig]) -> Vec<&ToolCallRequest> {
    history
        .iter()
        .filter_map(|entry| entry.event.as_tool_call_request())
        .collect()
}

/// The events of the response under assertion, the last one so far.
fn last_response(responses: &[Vec<TestEvent>]) -> &[TestEvent] {
    responses.last().map_or(&[], Vec::as_slice)
}

/// The tool call requests in one response.
fn requested(response: &[TestEvent]) -> Vec<&ToolCallRequest> {
    response
        .iter()
        .filter_map(|event| match event {
            TestEvent::Flushed(event) => event.as_tool_call_request(),
            TestEvent::Finished(_) => None,
        })
        .collect()
}

/// Answer every tool call request in the history with `result`, and require the
/// model to answer with a message.
///
/// [`TestRequest::tool_call_response`] answers the last request only, which
/// leaves the other calls of a response unanswered.
fn answer_every_tool_call(provider: ProviderId, result: &'static str) -> TestRequest {
    TestRequest::func(move |history| {
        let mut request = TestRequest::chat(provider);
        for call in tool_calls(history) {
            request = request.event(ToolCallResponse {
                id: call.id.clone(),
                result: Ok(result.to_owned()),
            });
        }

        // Asserted on the response rather than the history, so a failure shows
        // everything the provider sent back, finish reason included.
        Some(request.assert_chat(|responses| {
            let response = last_response(responses);
            let answer = response
                .iter()
                .rev()
                .find_map(|event| match event {
                    TestEvent::Flushed(event) => Some(event),
                    TestEvent::Finished(_) => None,
                })
                .and_then(|event| event.as_chat_response());
            assert!(
                matches!(answer, Some(ChatResponse::Message { message }) if !message.trim().is_empty()),
                "expected the model to answer once every tool call had a result, got: \
                 {response:#?}"
            );
        }))
    })
}

/// Providers whose test model makes one tool call per response, even when asked
/// for two.
///
/// Cerebras's `gpt-oss-120b` made one call in two recording runs, its reasoning
/// saying it can only call one function per turn.
/// Cerebras sends `parallel_tool_calls` on by default, so the request can't
/// change that.
const NO_PARALLEL_TOOL_CALLS: &[ProviderId] = &[ProviderId::Cerebras];

/// More than one tool call in a single response.
///
/// Each call has to keep its own arguments, and the provider has to accept one
/// result per call before the model answers.
async fn tool_call_parallel(
    provider: ProviderId,
    test_name: &str,
    mode: ProviderTestMode,
) -> Result {
    if !records_new_cases(provider, mode) || NO_PARALLEL_TOOL_CALLS.contains(&provider) {
        return Ok(());
    }

    let requests = vec![
        TestRequest::chat(provider)
            .chat_request(
                "Look up the weather in Paris and in Tokyo. Call `get_weather` once for each \
                 city, with both calls in the same response, before you answer.",
            )
            .tool(
                "get_weather",
                json!({
                    "type": "object",
                    "properties": {
                        "city": { "type": "string", "enum": ["Paris", "Tokyo"] }
                    },
                    "required": ["city"]
                }),
            )
            .assert_chat(|responses| {
                let response = last_response(responses);
                let mut cities: Vec<_> = requested(response)
                    .into_iter()
                    .map(|call| call.arguments.get("city").and_then(Value::as_str))
                    .collect();
                cities.sort_unstable();

                assert_eq!(
                    cities,
                    [Some("Paris"), Some("Tokyo")],
                    "expected one `get_weather` call per city in a single response, got: \
                     {response:#?}"
                );
            }),
        answer_every_tool_call(provider, "18 degrees and sunny"),
    ];

    run_test_mode(provider, test_name, requests, mode).await
}

/// A tool whose arguments nest objects inside an array.
///
/// This is the shape a tool with `fan_out` configured is advertised with: an
/// `ops` array holding one object of the tool's own arguments per operation.
async fn tool_call_nested_arguments(
    provider: ProviderId,
    test_name: &str,
    mode: ProviderTestMode,
) -> Result {
    if !records_new_cases(provider, mode) {
        return Ok(());
    }

    let requests = vec![
        TestRequest::chat(provider)
            // The follow-up turn requires an answer, so the prompt asks for
            // one: asked only to read the files, Gemini answered with nothing.
            .chat_request(
                "Read `a.txt` and `b.txt` with a single `read_files` call that lists both files \
                 in `ops`, then tell me what each file contains.",
            )
            .tool(
                "read_files",
                json!({
                    "type": "object",
                    "properties": {
                        "ops": {
                            "type": "array",
                            "minItems": 1,
                            "description": "The files to read, one element per file.",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "path": { "type": "string", "enum": ["a.txt", "b.txt"] },
                                    "start_line": { "type": "integer" }
                                },
                                "required": ["path"]
                            }
                        }
                    },
                    "required": ["ops"],
                    "additionalProperties": false
                }),
            )
            .assert_history(|history| {
                let ops: Vec<_> = tool_calls(history)
                    .into_iter()
                    .filter_map(|call| call.arguments.get("ops").and_then(Value::as_array))
                    .flatten()
                    .collect();

                let mut paths: Vec<_> = ops
                    .iter()
                    .map(|op| op.get("path").and_then(Value::as_str))
                    .collect();
                paths.sort_unstable();
                assert_eq!(
                    paths,
                    [Some("a.txt"), Some("b.txt")],
                    "expected `ops` to name both files, got: {ops:?}"
                );

                // A provider that encodes an optional property as nullable
                // receives `null` for one the model left out. The tool must
                // not: `null` is not an integer.
                assert!(
                    ops.iter()
                        .all(|op| op.get("start_line") != Some(&Value::Null)),
                    "an omitted `start_line` arrived as null: {ops:?}"
                );
            }),
        answer_every_tool_call(provider, "a.txt: alpha\nb.txt: beta"),
    ];

    run_test_mode(provider, test_name, requests, mode).await
}

/// A tool call whose result is an error.
///
/// The cassette pins how the provider's request encodes a failed result, and
/// that the provider accepts it and the model answers.
async fn tool_call_error(provider: ProviderId, test_name: &str, mode: ProviderTestMode) -> Result {
    if !records_new_cases(provider, mode) {
        return Ok(());
    }

    let requests = vec![
        tool_call_base(provider).tool_choice_fn("run_me"),
        TestRequest::tool_call_response(Err("`run_me` failed: the disk is full."), true),
    ];

    run_test_mode(provider, test_name, requests, mode).await
}

/// A tool that takes no arguments, so a call to it carries none.
async fn tool_call_without_parameters(
    provider: ProviderId,
    test_name: &str,
    mode: ProviderTestMode,
) -> Result {
    if !records_new_cases(provider, mode) {
        return Ok(());
    }

    let requests = vec![
        TestRequest::chat(provider)
            .chat_request("Call the `ping` tool.")
            .tool_without_parameters("ping")
            .tool_choice_fn("ping")
            .assert_history(|history| {
                let calls: Vec<_> = tool_calls(history)
                    .into_iter()
                    .map(|call| (call.name.as_str(), &call.arguments))
                    .collect();

                assert_eq!(calls, [("ping", &Map::new())]);
            }),
        TestRequest::tool_call_response(Ok("pong"), true),
    ];

    run_test_mode(provider, test_name, requests, mode).await
}

/// Providers that resolve `model_details` locally from a built-in table without
/// sending an HTTP request.
const LOCAL_MODEL_DETAILS: &[ProviderId] = &[ProviderId::Openai];

async fn model_details(provider: ProviderId, test_name: &str, mode: ProviderTestMode) -> Result {
    if LOCAL_MODEL_DETAILS.contains(&provider) {
        return Ok(());
    }

    let request = TestRequest::ModelDetails {
        name: test_model_details(provider).id.name.to_string(),
        assert: Arc::new(|_| {}),
    };

    run_test_mode(provider, test_name, Some(request), mode).await
}

async fn models(provider: ProviderId, test_name: &str, mode: ProviderTestMode) -> Result {
    let request = TestRequest::Models {
        assert: Arc::new(|_| {}),
    };

    run_test_mode(provider, test_name, Some(request), mode).await
}

const UNKNOWN_MODEL: &str = "foobarbaz";

/// Providers that ignore the requested `model` field and serve whichever model
/// the server was started with.
const SERVES_ANY_MODEL: &[ProviderId] = &[ProviderId::Llamacpp];

/// Verify that a chat request for an unknown model fails with
/// [`StreamErrorKind::UnknownModel`].
///
/// Clones the route's valid [`ModelDetails`] and replaces only the model name
/// so the request is not rejected for unrelated parameter mismatches.
async fn unknown_model(provider: ProviderId, test_name: &str, mode: ProviderTestMode) -> Result {
    if !records_new_cases(provider, mode) {
        return Ok(());
    }

    let support = provider_test_support(provider);
    let mut details = match mode {
        ProviderTestMode::Subscription => support.subscription().map(ProviderTestRoute::model),
        ProviderTestMode::Api => None,
    }
    .unwrap_or_else(|| support.api().model());
    details.id.name = UNKNOWN_MODEL.parse()?;

    let request = TestRequest::chat(provider)
        .model(details.id.clone())
        .model_details(details)
        .chat_request("Test message")
        .expect_refusal(move |refusal| {
            if SERVES_ANY_MODEL.contains(&provider) {
                assert!(
                    matches!(refusal, TestRefusal::Answered),
                    "expected {provider} to answer for any model, got: {refusal:?}"
                );
                return;
            }

            assert!(
                matches!(refusal, TestRefusal::Stream {
                    kind: StreamErrorKind::UnknownModel,
                    ..
                }),
                "expected the stream to refuse `{UNKNOWN_MODEL}` as an unknown model, got: \
                 {refusal:?}"
            );
        });

    run_test_mode(provider, test_name, Some(request), mode).await
}

/// Verify that [`Provider::model_details`] returns empty [`ModelDetails`] for
/// an unknown model, deferring unknown-model refusal to the chat request.
async fn unknown_model_details(
    provider: ProviderId,
    test_name: &str,
    mode: ProviderTestMode,
) -> Result {
    if LOCAL_MODEL_DETAILS.contains(&provider) {
        return Ok(());
    }

    let expected = ModelDetails::empty((provider, UNKNOWN_MODEL).try_into()?);
    let request = TestRequest::ModelLookup {
        name: UNKNOWN_MODEL.to_owned(),
        assert: Arc::new(move |result| {
            assert_eq!(
                result,
                &Ok(expected.clone()),
                "unexpected lookup result for `{UNKNOWN_MODEL}`: {result:?}"
            );
        }),
    };

    run_test_mode(provider, test_name, Some(request), mode).await
}

/// Providers that don't support image/vision input.
const NO_IMAGE_SUPPORT: &[ProviderId] = &[ProviderId::Cerebras];

async fn image_attachment(provider: ProviderId, test_name: &str, mode: ProviderTestMode) -> Result {
    if NO_IMAGE_SUPPORT.contains(&provider) {
        return Ok(());
    }

    let request = TestRequest::chat(provider)
        .attachment(fixture_attachment("banana.jpg"))
        .event(ChatRequest::from(
            "What fruit is in this image? Answer with just the fruit name, nothing else.",
        ))
        .assert_history(|history| {
            let response_text: String = history
                .iter()
                .filter_map(|e| e.event.as_chat_response())
                .filter_map(|r| match r {
                    jp_conversation::event::ChatResponse::Message { message } => {
                        Some(message.as_str())
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("")
                .to_lowercase();

            assert!(
                response_text.contains("apple"),
                "Expected the model to identify an apple, got: {response_text:?}"
            );
        });

    run_test_mode(provider, test_name, Some(request), mode).await
}

async fn structured_output(
    provider: ProviderId,
    test_name: &str,
    mode: ProviderTestMode,
) -> Result {
    let schema = crate::title::title_schema(1);

    let request = TestRequest::chat(provider).chat_request(ChatRequest {
        content: "Generate a title for this conversation.".into(),
        schema: Some(schema),
        author: None,
    });

    run_test_mode(provider, test_name, Some(request), mode).await
}

async fn multi_turn_conversation(
    provider: ProviderId,
    test_name: &str,
    mode: ProviderTestMode,
) -> Result {
    let requests = vec![
        TestRequest::chat(provider).chat_request("Test message"),
        TestRequest::chat(provider)
            .enable_reasoning()
            .chat_request("Repeat my previous message"),
        tool_call_base(provider).tool_choice_fn("run_me"),
        TestRequest::tool_call_response(Ok("The secret code is: 42"), true),
        TestRequest::chat(provider)
            .enable_reasoning()
            .chat_request("What was the result of the previous tool call?"),
    ];

    run_test_mode(provider, test_name, requests, mode).await
}

#[test]
fn trace_to_tmpfile_writes_distinct_files() {
    // Successive calls within a process must not clobber each other, so an
    // intermittent failure keeps the payload of every request it made.
    let first = trace_to_tmpfile("jp-test-trace", &1);
    let second = trace_to_tmpfile("jp-test-trace", &2);

    assert_ne!(first, second);
    assert_eq!(std::fs::read_to_string(&first).unwrap(), "1");
    assert_eq!(std::fs::read_to_string(&second).unwrap(), "2");

    std::fs::remove_file(&first).ok();
    std::fs::remove_file(&second).ok();
}

/// `preflight` must surface missing credentials locally, without any I/O, so
/// callers can fail fast before starting side-effectful work (spawning title
/// generation, loading attachments) that is wasted when the request can never
/// be sent.
#[test]
fn preflight_reports_missing_credentials() {
    // Point at an environment variable that is guaranteed unset, so the
    // outcome doesn't depend on the developer's shell.
    let mut config = LlmProviderConfig::default();
    config.openrouter.api_key_env = "JP_TEST_PREFLIGHT_UNSET_VAR".into();

    let error = preflight(ProviderId::Openrouter, &config).unwrap_err();
    assert!(
        matches!(
            &error,
            crate::Error::MissingEnv(var) if var == "JP_TEST_PREFLIGHT_UNSET_VAR"
        ),
        "expected MissingEnv, got: {error:?}"
    );
}

#[test]
fn preflight_passes_when_credentials_present() {
    // Mirrors the dummy-key handling in `compaction_request_tests`: `USER`
    // (or `USERNAME` on Windows) is always set.
    let env = if cfg!(windows) { "USERNAME" } else { "USER" }.to_owned();
    let mut config = LlmProviderConfig::default();
    config.openrouter.api_key_env = env.into();

    preflight(ProviderId::Openrouter, &config).unwrap();
    // Llamacpp requires no credentials at all.
    preflight(ProviderId::Llamacpp, &config).unwrap();
}

/// A provider billed one way skips its subscription suite, which is silent by
/// design.
/// That silence would also swallow a provider losing a route it does have,
/// turning its recorded cases into passing no-ops, so the routes that are
/// supposed to exist are asserted here.
#[test]
fn providers_offer_the_routes_their_suites_record() {
    for (id, plan) in [
        (ProviderId::Anthropic, "Claude Pro/Max"),
        (ProviderId::Openai, "a ChatGPT plan"),
    ] {
        let support = crate::provider::provider_test_support(id);
        assert!(
            support.subscription().is_some(),
            "{id} sells {plan}, so its subscription suite must run"
        );
    }
}

// Recording a subscription suite authenticates through the credential store,
// so `jp provider llm auth login <provider>` once is the whole setup and nothing
// has to be exported into the environment.
//
// Record through `just test`, serially (`-j1`) so each fixture captures one
// isolated exchange, and with `--no-fail-fast` so one failing provider does not
// leave the rest unrecorded:
//
//     RECORD=1 just test -p jp_llm -j1 --no-fail-fast \
//       provider::tests::openai_subscription::
//
// A bare `cargo test` skips the `INSTA_WORKSPACE_ROOT` that `just test`
// exports. Without it, a test binary shared between git worktrees looks for
// `.config/insta.yaml` in the worktree that built it, new snapshots are left as
// failing `.snap.new` files, and the cassette is never saved.
//
// A self-hosted provider (llama.cpp, Ollama, vLLM) records against its default
// loopback address. To record one against a server elsewhere, set the route's
// own variable, such as `JP_TEST_UPSTREAM_VLLM=http://model.api:8001`. No other
// route reads it, so it can stay exported while every provider records in one
// run.
test_all_providers![
    every_route: [
        chat_completion_stream,
        image_attachment,
        tool_call_auto,
        tool_call_function,
        tool_call_reasoning,
        tool_call_required_no_reasoning,
        tool_call_required_reasoning,
        tool_call_stream,
        tool_call_parallel,
        tool_call_nested_arguments,
        tool_call_error,
        tool_call_without_parameters,
        multi_turn_conversation,
        structured_output,
        unknown_model,
    ],
    api_only: [model_details, models, unknown_model_details],
];
