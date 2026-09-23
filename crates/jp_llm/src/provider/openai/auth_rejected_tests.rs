//! A credential the host refuses moves the request onto the next one.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use futures::StreamExt as _;
use jp_config::{AppConfig, providers::llm::AuthEntry, types::api_key_env::ApiKeyEnv};
use jp_conversation::{ConversationStream, thread::Thread};
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{TcpListener, TcpStream},
};

use super::{Openai, Provider as _, STREAMING_UNSUPPORTED};
use crate::{StreamError, StreamErrorKind, model::ModelDetails, query::ChatQuery};

/// Two variables the environment always holds, standing in for two API keys.
const WORK_KEY_ENV: &str = if cfg!(windows) { "USERNAME" } else { "USER" };
const PERSONAL_KEY_ENV: &str = if cfg!(windows) { "USERPROFILE" } else { "HOME" };

/// The body OpenAI answers a revoked or mistyped key with.
const INVALID_KEY: &str = r#"{"error":{"message":"Incorrect API key provided.","type":"invalid_request_error","code":"invalid_api_key"}}"#;

/// Answer every request with `401`, recording the `Authorization` header each
/// one carried.
async fn refuse_everything(listener: TcpListener, seen: Arc<Mutex<Vec<String>>>) {
    loop {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };

        let head = read_head(&mut socket).await;
        let authorization = head
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("authorization")
                    .then(|| value.trim().to_owned())
            })
            .unwrap_or_default();
        seen.lock().unwrap().push(authorization);

        let response = format!(
            "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: \
             {}\r\nConnection: close\r\n\r\n{INVALID_KEY}",
            INVALID_KEY.len()
        );
        socket.write_all(response.as_bytes()).await.unwrap();
    }
}

/// Read a request up to the end of its headers.
///
/// The body is left unread; the response does not depend on it.
async fn read_head(socket: &mut TcpStream) -> String {
    let mut raw = vec![];
    let mut chunk = [0u8; 4096];

    loop {
        let read = socket.read(&mut chunk).await.unwrap();
        raw.extend_from_slice(&chunk[..read]);

        let text = String::from_utf8_lossy(&raw).to_string();
        if let Some((head, _)) = text.split_once("\r\n\r\n") {
            return head.to_owned();
        }
        if read == 0 {
            return text;
        }
    }
}

/// Send one request for `model` through a chain of two keys the server refuses,
/// returning the `Authorization` headers it saw and the error the caller got.
async fn refused_twice(model: ModelDetails) -> (Vec<String>, StreamError) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(vec![]));
    let server = tokio::spawn(refuse_everything(listener, seen.clone()));

    let mut config = AppConfig::new_test().providers.llm.openai;
    config.base_url = format!("http://{address}");
    config.base_url_env = "JP_TEST_OPENAI_BASE_URL_UNSET".to_owned();
    config.auth = vec![
        AuthEntry::ApiKey(Some("work".to_owned())),
        AuthEntry::ApiKey(Some("personal".to_owned())),
    ];
    config.api_key_env = ApiKeyEnv::Many(BTreeMap::from([
        ("work".to_owned(), WORK_KEY_ENV.to_owned()),
        ("personal".to_owned(), PERSONAL_KEY_ENV.to_owned()),
    ]));

    let provider = Openai::new(&config).unwrap();
    let query = ChatQuery::from(Thread {
        system_prompt: None,
        sections: vec![],
        attachments: vec![],
        events: ConversationStream::new_test().with_turn("hello"),
    });

    let events: Vec<_> = provider
        .chat_completion_stream(&model, query)
        .await
        .unwrap()
        .collect()
        .await;
    server.abort();

    let error = events
        .into_iter()
        .find_map(Result::err)
        .expect("the request fails once every key is refused");
    let seen = seen.lock().unwrap().clone();

    (seen, error)
}

/// The headers both keys should have been sent with, in chain order.
fn both_keys() -> [String; 2] {
    [
        format!("Bearer {}", std::env::var(WORK_KEY_ENV).unwrap()),
        format!("Bearer {}", std::env::var(PERSONAL_KEY_ENV).unwrap()),
    ]
}

#[tokio::test]
async fn test_a_refused_api_key_falls_through_to_the_next_one() {
    let model = ModelDetails::empty("openai/gpt-5.6-sol".parse().unwrap());

    let (seen, error) = refused_twice(model).await;

    // Both keys were tried, in chain order, and neither twice.
    assert_eq!(seen, both_keys());

    // With nothing left in the chain, the refusal is what the caller sees.
    assert_eq!(error.kind, StreamErrorKind::AuthRejected, "{error}");
}

/// A model served without streaming reaches the host through a different client
/// call, which reports a refusal its own way.
#[tokio::test]
async fn test_a_refused_api_key_falls_through_without_streaming() {
    let mut model = ModelDetails::empty("openai/gpt-5.5-pro".parse().unwrap());
    model.features = vec![STREAMING_UNSUPPORTED];

    let (seen, error) = refused_twice(model).await;

    assert_eq!(seen, both_keys());
    assert_eq!(error.kind, StreamErrorKind::AuthRejected, "{error}");
}
