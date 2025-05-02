use std::{env, fs, path::PathBuf};

use futures::StreamExt as _;
use httpmock::{MockServer, RecordingRuleBuilder};
use jp_openrouter::{
    types::{
        chat::Message,
        request::{self, RequestMessage},
        response,
    },
    Client,
};

/// Get the name of the calling function using Rust's type system
#[macro_export]
macro_rules! function_name {
    () => {{
        fn f() {}
        fn type_name_of<T>(_: T) -> &'static str {
            std::any::type_name::<T>()
        }
        let name = type_name_of(f);
        let name = name.trim_end_matches("::{{closure}}::f");
        match &name.rfind(':') {
            Some(pos) => &name[pos + 1..name.len()],
            None => &name[..name.len()],
        }
    }};
}

/// Run a test with HTTP recording/playback capabilities
///
/// The recording logic is fully encapsulated in this function.
/// The caller only needs to provide:
/// 1. A rule builder function to configure the recording
/// 2. A test function that runs the actual test
///
/// # Panics
///
/// ...
pub async fn with_recording<R, F, Fut>(
    scenario: &str,
    forward_to: &str,
    rule_builder: R,
    test_fn: F,
) where
    R: FnOnce(RecordingRuleBuilder),
    F: FnOnce(bool, String) -> Fut,
    Fut: Future<Output = ()>,
{
    let should_record = env::var("RECORD").is_ok();
    let fixtures_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    fs::create_dir_all(&fixtures_dir).expect("Failed to create fixtures directory");

    let fixture_path = fixtures_dir.join(format!("{scenario}.yml"));
    let server = MockServer::start_async().await;

    if should_record {
        server
            .forward_to_async(forward_to, |rule| {
                rule.filter(|when| {
                    when.any_request();
                });
            })
            .await;

        let recording = server.record_async(rule_builder).await;

        test_fn(true, server.base_url()).await;

        let temp_path = recording
            .save_to_async(&fixtures_dir, scenario)
            .await
            .expect("Failed to save recording");

        if temp_path != fixture_path && temp_path.exists() {
            fs::rename(temp_path, &fixture_path)
                .expect("Failed to move recording file to final location");
        }
    } else {
        assert!(
            fixture_path.exists(),
            "Recording not found at {}. Run with RECORD=1 to create it.",
            fixture_path.display()
        );

        server.playback_async(&fixture_path).await;
        test_fn(false, server.base_url()).await;
    }
}

#[tokio::test]
async fn test_chat_completion_stream() {
    let sample_request = request::ChatCompletion {
        model: "anthropic/claude-3-haiku".to_string(),
        messages: vec![RequestMessage::User(
            Message::default().with_text("Tell me a short story."),
        )],
        ..Default::default()
    };

    with_recording(
        function_name!(),
        "https://openrouter.ai",
        |rule| {
            rule.filter(|when| {
                when.any_request();
            });
        },
        // Create a client that points to the mock server
        |recording, url| async move {
            let api_key = recording
                .then(|| env::var("OPENROUTER_API_KEY").ok())
                .flatten()
                .unwrap_or_default();

            // Make the request
            let stream = Client::new(api_key, None, None)
                .with_base_url(url)
                .chat_completion_stream(&sample_request);

            // Collect all chunks from the stream
            let mut collected_chunks: Vec<response::ChatCompletion> = Vec::new();
            let mut stream = Box::pin(stream);

            while let Some(result) = stream.next().await {
                match result {
                    Ok(chunk) => {
                        collected_chunks.push(chunk);
                    }
                    Err(e) => panic!("Stream error during test: {e}"),
                }
            }

            assert!(
                !collected_chunks.is_empty(),
                "Should receive at least one chunk"
            );
        },
    )
    .await;
}
