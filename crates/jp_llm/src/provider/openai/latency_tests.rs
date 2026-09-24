//! The provider constructs a stream without waiting for its first event.

use std::time::Duration;

use jp_config::AppConfig;
use jp_conversation::{ConversationStream, thread::Thread};
use tokio::net::TcpListener;

use super::{Openai, Provider as _};
use crate::{credential::Credential, model::ModelDetails, query::ChatQuery};

#[tokio::test]
async fn test_chat_completion_returns_before_the_first_response_event() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        // Holding the accepted socket open reproduces model time-to-first-token:
        // the request has reached the server, but no response event exists yet.
        let (_socket, _) = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_millis(250)).await;
    });

    let mut config = AppConfig::new_test().providers.llm.openai;
    config.codex_base_url = format!("http://{address}");
    let provider = Openai::with_credential(&config, Credential::Bearer("token".to_owned()));
    let model = ModelDetails::empty("openai/gpt-5.6-sol".parse().unwrap());
    let query = ChatQuery::from(Thread {
        system_prompt: None,
        sections: vec![],
        attachments: vec![],
        events: ConversationStream::new_test().with_turn("hello"),
    });

    let result = tokio::time::timeout(
        Duration::from_millis(50),
        provider.chat_completion_stream(&model, query),
    )
    .await;
    server.abort();

    assert!(
        result.is_ok(),
        "constructing the stream waited for the provider's first response event"
    );
    assert!(result.unwrap().is_ok());
}
