use jp_config::providers::llm::LlmProviderConfig;
use jp_test::{Result, function_name};

use super::*;
use crate::test::TestRequest;

macro_rules! test_all_models {
        ($($fn:ident),* $(,)?) => {
            mod anthropic { use super::*; $(test_all_models!(func; $fn, "openrouter/anthropic/claude-haiku-4.5");)* }
            mod google    { use super::*; $(test_all_models!(func; $fn, "openrouter/google/gemini-2.5-flash");)* }
            mod xai       { use super::*; $(test_all_models!(func; $fn, "openrouter/x-ai/grok-code-fast-1");)* }
            mod minimax   { use super::*; $(test_all_models!(func; $fn, "openrouter/minimax/minimax-m2");)* }
        };
        (func; $fn:ident, $model:literal) => {
            paste::paste! {
                #[test_log::test(tokio::test)]
                async fn [< test_ $fn >]() -> Result {
                    $fn($model, &format!("{}_{}", $model.split('/').nth(1).unwrap(), function_name!())).await
                }
            }
        };
    }

test_all_models![sub_provider_event_metadata];

async fn sub_provider_event_metadata(model: &str, test_name: &str) -> Result {
    let requests = vec![
        TestRequest::chat(ProviderId::Openrouter)
            .model(model.parse().unwrap())
            .enable_reasoning()
            .chat_request("Test message"),
    ];

    run_test(test_name, requests).await?;

    Ok(())
}

async fn run_test(
    test_name: impl AsRef<str>,
    requests: impl IntoIterator<Item = TestRequest>,
) -> Result {
    crate::test::run_chat_completion(
        test_name,
        env!("CARGO_MANIFEST_DIR"),
        ProviderId::Openrouter,
        LlmProviderConfig::default(),
        requests.into_iter().collect(),
    )
    .await
}
