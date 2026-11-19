//! The request body the subscription route puts on the wire, captured by a
//! local stand-in for the subscription host.

use futures::StreamExt as _;
use jp_config::{AppConfig, assistant::request::CachePolicy};
use jp_conversation::{ConversationStream, thread::Thread};
use serde_json::{Value, json};
use tokio::{io::AsyncWriteExt as _, net::TcpListener};

use super::{
    Openai, Provider as _,
    test_support::{catalog_model_details, read_request},
};
use crate::query::ChatQuery;

/// Send one query with `cache` to `model` through the subscription route and
/// return the captured request body.
async fn subscription_body(model: &str, cache: CachePolicy) -> Value {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let host = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let (_, body) = read_request(&mut socket).await;
        let reply = r#"{"error":{"type":"invalid_request_error","message":"stand-in"}}"#;
        let response = format!(
            "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: \
             {}\r\nConnection: close\r\n\r\n{reply}",
            reply.len()
        );
        socket.write_all(response.as_bytes()).await.unwrap();
        body
    });

    let mut app = AppConfig::new_test();
    app.assistant.request.cache = cache;
    let mut config = app.providers.llm.openai.clone();
    config.codex_base_url = format!("http://{address}/codex");
    let provider = Openai::with_subscription_credential(
        &config,
        "bearer-token".to_owned(),
        "acct-1".to_owned(),
    );

    let query = ChatQuery::from(Thread {
        system_prompt: None,
        sections: vec![],
        attachments: vec![],
        events: ConversationStream::new(app.into()).with_turn("hello"),
    });

    let stream = provider
        .chat_completion_stream(&catalog_model_details(model), query)
        .await
        .unwrap();
    let _events: Vec<_> = stream.collect().await;

    serde_json::from_str(&host.await.unwrap()).unwrap()
}

/// The subscription host takes a cache lifetime the way Codex sends it: `ttl`
/// alone, without a mode.
#[tokio::test]
async fn long_cache_sends_a_12_hour_ttl() {
    let body = subscription_body("gpt-6.2-sol", CachePolicy::Long).await;

    assert_eq!(body["prompt_cache_options"], json!({ "ttl": "12h" }));
}

#[tokio::test]
async fn short_cache_sends_no_cache_options() {
    let body = subscription_body("gpt-6.2-sol", CachePolicy::Short).await;

    assert_eq!(body.get("prompt_cache_options"), None);
}

/// Disabling caching requires `mode: explicit`, which the subscription host
/// does not support, so `prompt_cache_options` is omitted.
#[tokio::test]
async fn off_cache_sends_no_cache_options() {
    let body = subscription_body("gpt-6.2-sol", CachePolicy::Off).await;

    assert_eq!(body.get("prompt_cache_options"), None);
}
