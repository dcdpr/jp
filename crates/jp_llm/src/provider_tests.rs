use std::sync::Arc;

use indexmap::IndexMap;
use jp_config::{
    assistant::tool_choice::ToolChoice,
    conversation::tool::{OneOrManyTypes, ToolParameterConfig},
};
use jp_conversation::event::ChatRequest;
use jp_test::{Result, function_name};

use super::*;
use crate::test::{TestRequest, run_test, test_model_details};

macro_rules! test_all_providers {
        ($($fn:ident),* $(,)?) => {
            mod anthropic { use super::*; $(test_all_providers!(func; $fn, ProviderId::Anthropic);)* }
            mod google    { use super::*; $(test_all_providers!(func; $fn, ProviderId::Google);)* }
            mod openai    { use super::*; $(test_all_providers!(func; $fn, ProviderId::Openai);)* }
            mod openrouter{ use super::*; $(test_all_providers!(func; $fn, ProviderId::Openrouter);)* }
            mod ollama    { use super::*; $(test_all_providers!(func; $fn, ProviderId::Ollama);)* }
            mod llamacpp  { use super::*; $(test_all_providers!(func; $fn, ProviderId::Llamacpp);)* }
        };
        (func; $fn:ident, $provider:ty) => {
            paste::paste! {
                #[test_log::test(tokio::test)]
                async fn [< test_ $fn >]() -> Result {
                    $fn($provider, function_name!()).await
                }
            }
        };
    }

async fn chat_completion_stream(provider: ProviderId, test_name: &str) -> Result {
    let request = TestRequest::chat(provider)
        .enable_reasoning()
        .event(ChatRequest::from("Test message"));

    run_test(provider, test_name, Some(request)).await
}

fn tool_call_base(provider: ProviderId) -> TestRequest {
    TestRequest::chat(provider)
        .event(ChatRequest::from(
            "Please run the tool, providing whatever arguments you want.",
        ))
        .tool("run_me", vec![
            ("foo", ToolParameterConfig {
                kind: OneOrManyTypes::One("string".into()),
                default: Some("foo".into()),
                required: false,
                summary: None,
                description: None,
                examples: None,
                enumeration: vec![],
                items: None,
                properties: IndexMap::default(),
            }),
            ("bar", ToolParameterConfig {
                kind: OneOrManyTypes::Many(vec!["string".into(), "array".into()]),
                default: None,
                required: true,
                summary: None,
                description: None,
                examples: None,
                enumeration: vec!["foo".into(), vec!["foo", "bar"].into()],
                items: Some(Box::new(ToolParameterConfig {
                    kind: OneOrManyTypes::One("string".into()),
                    default: None,
                    required: false,
                    summary: None,
                    description: None,
                    examples: None,
                    enumeration: vec![],
                    items: None,
                    properties: IndexMap::default(),
                })),
                properties: IndexMap::default(),
            }),
        ])
}

async fn tool_call_stream(provider: ProviderId, test_name: &str) -> Result {
    let requests = vec![
        tool_call_base(provider),
        TestRequest::tool_call_response(Ok("working!"), false),
    ];

    run_test(provider, test_name, requests).await
}

/// Without reasoning, "forced" tool calls should work as expected.
async fn tool_call_required_no_reasoning(provider: ProviderId, test_name: &str) -> Result {
    let requests = vec![
        tool_call_base(provider).tool_choice(ToolChoice::Required),
        TestRequest::tool_call_response(Ok("working!"), true),
    ];

    run_test(provider, test_name, requests).await
}

/// With reasoning, some models do not support "forced" tool calls, so
/// provider implementations should fall back to trying to instruct the
/// model to use the tool through regular textual instructions.
async fn tool_call_required_reasoning(provider: ProviderId, test_name: &str) -> Result {
    let requests = vec![
        tool_call_base(provider)
            .tool_choice(ToolChoice::Required)
            .enable_reasoning(),
        TestRequest::tool_call_response(Ok("working!"), false),
    ];

    run_test(provider, test_name, requests).await
}

async fn tool_call_auto(provider: ProviderId, test_name: &str) -> Result {
    let requests = vec![
        tool_call_base(provider).tool_choice(ToolChoice::Auto),
        TestRequest::tool_call_response(Ok("working!"), false),
    ];

    run_test(provider, test_name, requests).await
}

async fn tool_call_function(provider: ProviderId, test_name: &str) -> Result {
    let requests = vec![
        tool_call_base(provider).tool_choice_fn("run_me"),
        TestRequest::tool_call_response(Ok("working!"), true),
    ];

    run_test(provider, test_name, requests).await
}

async fn tool_call_reasoning(provider: ProviderId, test_name: &str) -> Result {
    let requests = vec![
        tool_call_base(provider).enable_reasoning(),
        TestRequest::tool_call_response(Ok("working!"), false),
    ];

    run_test(provider, test_name, requests).await
}

async fn model_details(provider: ProviderId, test_name: &str) -> Result {
    let request = TestRequest::ModelDetails {
        name: test_model_details(provider).id.name.to_string(),
        assert: Arc::new(|_| {}),
    };

    run_test(provider, test_name, Some(request)).await
}

async fn models(provider: ProviderId, test_name: &str) -> Result {
    let request = TestRequest::Models {
        assert: Arc::new(|_| {}),
    };

    run_test(provider, test_name, Some(request)).await
}

async fn structured_output(provider: ProviderId, test_name: &str) -> Result {
    let schema = crate::title::title_schema(1);

    let request = TestRequest::chat(provider).chat_request(ChatRequest {
        content: "Generate a title for this conversation.".into(),
        schema: Some(schema),
    });

    run_test(provider, test_name, Some(request)).await
}

async fn multi_turn_conversation(provider: ProviderId, test_name: &str) -> Result {
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

    run_test(provider, test_name, requests).await
}

test_all_providers![
    chat_completion_stream,
    tool_call_auto,
    tool_call_function,
    tool_call_reasoning,
    tool_call_required_no_reasoning,
    tool_call_required_reasoning,
    tool_call_stream,
    model_details,
    models,
    multi_turn_conversation,
    structured_output,
];
