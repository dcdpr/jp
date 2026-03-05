use futures::StreamExt;
use jp_conversation::{ConversationStream, thread::Thread};

use super::*;

fn empty_query() -> ChatQuery {
    ChatQuery {
        thread: Thread {
            system_prompt: None,
            sections: vec![],
            attachments: vec![],
            events: ConversationStream::new_test(),
        },
        tools: vec![],
        tool_choice: jp_config::assistant::tool_choice::ToolChoice::Auto,
    }
}

fn test_name(s: &str) -> Name {
    s.parse().expect("valid test name")
}

#[tokio::test]
async fn test_with_message() {
    let provider = MockProvider::with_message("Hello, world!");
    let model = provider.model_details(&test_name("test")).await.unwrap();

    let mut stream = provider
        .chat_completion_stream(&model, empty_query())
        .await
        .unwrap();

    // First event: Part with message
    let event = stream.next().await.unwrap().unwrap();
    assert!(matches!(event, Event::Part { index: 0, .. }));

    // Second event: Flush
    let event = stream.next().await.unwrap().unwrap();
    assert!(matches!(event, Event::Flush { index: 0, .. }));

    // Third event: Finished
    let event = stream.next().await.unwrap().unwrap();
    assert!(matches!(event, Event::Finished(FinishReason::Completed)));

    // Stream should be exhausted
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn test_with_chunked_message() {
    let provider = MockProvider::with_chunked_message(&["Hello, ", "world", "!"]);
    let model = provider.model_details(&test_name("test")).await.unwrap();

    let mut stream = provider
        .chat_completion_stream(&model, empty_query())
        .await
        .unwrap();

    // Three Part events
    for _ in 0..3 {
        let event = stream.next().await.unwrap().unwrap();
        assert!(matches!(event, Event::Part { index: 0, .. }));
    }

    // Flush
    let event = stream.next().await.unwrap().unwrap();
    assert!(matches!(event, Event::Flush { index: 0, .. }));

    // Finished
    let event = stream.next().await.unwrap().unwrap();
    assert!(matches!(event, Event::Finished(FinishReason::Completed)));
}

#[tokio::test]
async fn test_with_reasoning_and_message() {
    let provider = MockProvider::with_reasoning_and_message("thinking...", "done");
    let model = provider.model_details(&test_name("test")).await.unwrap();

    let mut stream = provider
        .chat_completion_stream(&model, empty_query())
        .await
        .unwrap();

    // Reasoning part at index 0
    let event = stream.next().await.unwrap().unwrap();
    assert!(matches!(event, Event::Part { index: 0, .. }));

    // Flush index 0
    let event = stream.next().await.unwrap().unwrap();
    assert!(matches!(event, Event::Flush { index: 0, .. }));

    // Message part at index 1
    let event = stream.next().await.unwrap().unwrap();
    assert!(matches!(event, Event::Part { index: 1, .. }));

    // Flush index 1
    let event = stream.next().await.unwrap().unwrap();
    assert!(matches!(event, Event::Flush { index: 1, .. }));

    // Finished
    let event = stream.next().await.unwrap().unwrap();
    assert!(matches!(event, Event::Finished(FinishReason::Completed)));
}

#[tokio::test]
async fn test_model_details() {
    let provider = MockProvider::with_message("test");
    let model = provider
        .model_details(&test_name("custom-name"))
        .await
        .unwrap();

    assert_eq!(model.id.name.as_ref(), "custom-name");
    assert_eq!(model.id.provider, ProviderId::Test);
}

#[tokio::test]
async fn test_models_list() {
    let provider = MockProvider::with_message("test").with_model_name("my-model");
    let models = provider.models().await.unwrap();

    assert_eq!(models.len(), 1);
    assert_eq!(models[0].id.name.as_ref(), "my-model");
}
