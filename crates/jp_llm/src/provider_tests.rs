use std::sync::Arc;

use jp_config::assistant::tool_choice::ToolChoice;
use jp_conversation::event::ChatRequest;
use jp_test::{Result, function_name};
use serde_json::json;

use super::*;
use crate::test::{
    ProviderTestMode, TestRequest, fixture_attachment, run_test_mode, test_model_details,
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

async fn model_details(provider: ProviderId, test_name: &str, mode: ProviderTestMode) -> Result {
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
// Record serially, so each fixture captures one isolated exchange:
//
//     RECORD=1 cargo test -p jp_llm \
//       'provider::tests::openai_subscription::' -- --test-threads=1
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
        multi_turn_conversation,
        structured_output,
    ],
    api_only: [model_details, models],
];
