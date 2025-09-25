use std::{env, path::PathBuf};

use jp_config::{
    model::{id::ProviderId, parameters::ParametersConfig},
    providers::llm::LlmProviderConfig,
};
use jp_conversation::{AssistantMessage, MessagePair, UserMessage};
use jp_llm::{provider::openrouter::Openrouter, structured};
use jp_test::{function_name, mock::Vcr};

fn vcr() -> Vcr {
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    Vcr::new("https://openrouter.ai", fixtures)
}

#[tokio::test]
async fn test_conversation_titles() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    // Create test data
    let model_id = "openrouter/openai/o3-mini-high".parse().unwrap();
    let mut config = LlmProviderConfig::default().openrouter;
    let message = UserMessage::Query("Test message".to_string());
    let history = vec![MessagePair::new(
        message,
        AssistantMessage::new(ProviderId::Openrouter),
    )];

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
            let query = structured::titles::titles(3, history, &[]).unwrap();
            structured::completion::<Vec<String>>(
                &provider,
                &model_id,
                &ParametersConfig::default(),
                query,
            )
            .await
            .unwrap();
        },
    )
    .await
}
