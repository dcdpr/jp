use std::env;

use futures::StreamExt as _;
use jp_openrouter::{
    Client,
    types::{
        chat::Message,
        request::{self, RequestMessage},
        response,
    },
};
use jp_test::{
    fn_name,
    mock::{Snap, Vcr},
};

fn vcr() -> Vcr {
    Vcr::new("https://openrouter.ai", env!("CARGO_MANIFEST_DIR"))
}

#[tokio::test]
#[test_log::test]
async fn test_chat_completion_stream() {
    let sample_request = request::ChatCompletion {
        model: "anthropic/claude-3-haiku".to_string(),
        messages: vec![RequestMessage::User(
            Message::default().with_text("Give me a fitting sonnet for this test."),
        )],
        ..Default::default()
    };

    let vcr = vcr();
    vcr.cassette(
        fn_name!(),
        |rule| {
            rule.filter(|when| {
                when.any_request();
            });
        },
        |recording, url| async move {
            let api_key = recording
                .then(|| env::var("OPENROUTER_API_KEY").ok())
                .flatten()
                .unwrap_or_default();

            // Make the request
            let stream = Client::new(api_key, None, None)
                .with_base_url(url)
                .chat_completion_stream(sample_request);

            // Collect all chunks from the stream
            let mut collected_chunks: Vec<response::ChatCompletion> = Vec::new();
            let mut stream = Box::pin(stream);

            while let Some(result) = stream.next().await {
                match result {
                    Ok(chunk) if collected_chunks.len() < 10 => {
                        collected_chunks.push(chunk);
                    }
                    Ok(_) => {}
                    Err(e) => panic!("Stream error during test: {e}"),
                }
            }

            vec![("", Snap::debug(collected_chunks))]
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
#[test_log::test]
async fn test_chat_completion() {
    let sample_request = request::ChatCompletion {
        model: "anthropic/claude-3-haiku".to_string(),
        messages: vec![RequestMessage::User(
            Message::default().with_text("Give me a fitting haiku for this test."),
        )],
        ..Default::default()
    };

    let vcr = vcr();
    vcr.cassette(
        fn_name!(),
        |rule| {
            rule.filter(|when| {
                when.any_request();
            });
        },
        |recording, url| async move {
            let api_key = recording
                .then(|| env::var("OPENROUTER_API_KEY").ok())
                .flatten()
                .unwrap_or_default();

            // Make the request
            let response = Client::new(api_key, None, None)
                .with_base_url(url)
                .chat_completion(sample_request)
                .await;

            vec![("", Snap::debug(response))]
        },
    )
    .await
    .unwrap();
}
