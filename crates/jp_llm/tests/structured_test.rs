use std::{env, path::PathBuf};

use jp_config::llm::{self, ProviderModelSlug};
use jp_conversation::{AssistantMessage, MessagePair, UserMessage};
use jp_llm::{provider::openrouter::Openrouter, structured_completion};
use jp_query::structured::conversation_titles;
use jp_test::{function_name, mock::Vcr};

fn vcr() -> Vcr {
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    Vcr::new("https://openrouter.ai", fixtures)
}

#[tokio::test]
async fn test_conversation_titles() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    // Create test data
    let model: ProviderModelSlug = "openrouter/openai/o3-mini-high".parse().unwrap();
    let mut config = llm::Config::default().provider.openrouter;

    let message = UserMessage::Query("Test message".to_string());
    let history = vec![MessagePair::new(message, AssistantMessage::default())];

    let vcr = vcr();
    vcr.cassette(
        function_name!(),
        |rule| {
            rule.filter(|when| {
                when.any_request();
            });
        },
        |recording, url| async move {
            config.base_url = url;
            if !recording {
                // dummy api key value when replaying a cassette
                config.api_key_env = "USER".to_owned();
            }

            let provider = Openrouter::try_from(&config).unwrap();
            let query = conversation_titles(3, history, &[]).unwrap();
            structured_completion::<Vec<String>>(&provider, &model.into(), query).await
        },
    )
    .await
}
