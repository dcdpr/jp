use std::{env, path::PathBuf};

use jp_config::llm::{self, ProviderModelSlug};
use jp_conversation::{AssistantMessage, MessagePair, UserMessage};
use jp_llm::{provider::openrouter::Openrouter, structured_completion};
use jp_query::structured::conversation_titles;
use jp_test::{function_name, mock::Vcr};

#[tokio::test]
async fn test_conversation_titles() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    // Create test data
    let model: ProviderModelSlug = "openrouter/openai/o3-mini-high".parse().unwrap();
    let mut config = llm::Config::default();

    let message = UserMessage::Query("Test message".to_string());
    let history = vec![MessagePair::new(message, AssistantMessage::default())];
    let recording = env::var("RECORD").is_ok();
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut vcr = Vcr::new("https://openrouter.ai", fixtures);
    vcr.set_recording(recording);

    vcr.cassette(
        function_name!(),
        |rule| {
            rule.filter(|when| {
                when.any_request();
            });
        },
        |_recording, url| async move {
            config.provider.openrouter.base_url = url;
            let provider = Openrouter::try_from(&config.provider.openrouter).unwrap();
            let query = conversation_titles(3, history, &[]).unwrap();
            let titles: Vec<String> = structured_completion(&provider, &model.into(), query)
                .await
                .unwrap();

            assert_eq!(titles.len(), 3);
        },
    )
    .await
}
