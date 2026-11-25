//! Which notices a provider repeats across the requests of a turn.
//!
//! A turn sends one request per tool-call cycle, all through one provider.
//! A notice restating a standing condition (a skipped credential, replayed
//! reasoning the API withholds again) is yielded by the first request only.
//! A notice reporting an event (a credential switch) is yielded every time.

use std::collections::BTreeMap;

use futures::StreamExt as _;
use jp_config::{
    AppConfig,
    providers::llm::{AuthEntry, openai::OpenaiConfig},
    types::api_key_env::ApiKeyEnv,
};
use jp_conversation::{ConversationStream, thread::Thread};
use serde_json::json;
use tokio::{io::AsyncWriteExt as _, net::TcpListener, task::JoinHandle};

use super::{Openai, Provider as _, STREAMING_UNSUPPORTED, test_support::read_raw_request};
use crate::{
    credential::Credential,
    event::{Event, FinishReason},
    model::ModelDetails,
    query::ChatQuery,
};

/// Two variables the environment always holds, standing in for two API keys.
const WORK_KEY_ENV: &str = if cfg!(windows) { "USERNAME" } else { "USER" };
const PERSONAL_KEY_ENV: &str = if cfg!(windows) { "USERPROFILE" } else { "HOME" };

/// A variable the environment never holds.
const UNSET_KEY_ENV: &str = "JP_TEST_OPENAI_NOTICE_KEY_UNSET";

/// A completed response that withheld the replayed reasoning items `removed`.
fn completed(removed: &[&str]) -> String {
    let transformations: Vec<_> = removed
        .iter()
        .map(|id| {
            json!({
                "id": id, "action": "removed", "reason": "prefix_mismatch", "type": "reasoning"
            })
        })
        .collect();

    let body = json!({
        "id": "resp_123", "created_at": 1_577_836_800, "status": "completed",
        "model": "gpt-6.2-sol", "metadata": {}, "parallel_tool_calls": true,
        "reasoning": {}, "temperature": 1.0, "text": {"format": {"type": "text"}},
        "tool_choice": "auto", "tools": [], "top_p": 1.0, "truncation": "disabled",
        "store": false, "output": [],
        "input_transformations": transformations,
    })
    .to_string();

    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: \
         close\r\n\r\n{body}",
        body.len()
    )
}

/// The response OpenAI answers a revoked or mistyped key with.
fn unauthorized() -> String {
    let body = r#"{"error":{"message":"Incorrect API key provided.","type":"invalid_request_error","code":"invalid_api_key"}}"#;

    format!(
        "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: \
         {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// Answer each request with what `respond` returns for the request's head and
/// the number of requests before it.
fn serve(
    listener: TcpListener,
    respond: impl Fn(&str, usize) -> String + Send + 'static,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut index = 0;
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };

            let (head, _) = read_raw_request(&mut socket).await;
            let response = respond(&head, index);
            index += 1;

            socket.write_all(response.as_bytes()).await.unwrap();
        }
    })
}

/// A config whose API requests go to `listener`, with a chain of two named keys
/// read from `work` and `personal`.
fn two_key_config(listener: &TcpListener, work: &str, personal: &str) -> OpenaiConfig {
    let mut config = AppConfig::new_test().providers.llm.openai;
    config.base_url = format!("http://{}", listener.local_addr().unwrap());
    config.base_url_env = "JP_TEST_OPENAI_BASE_URL_UNSET".to_owned();
    config.auth = vec![
        AuthEntry::ApiKey(Some("work".to_owned())),
        AuthEntry::ApiKey(Some("personal".to_owned())),
    ];
    config.api_key_env = ApiKeyEnv::Many(BTreeMap::from([
        ("work".to_owned(), work.into()),
        ("personal".to_owned(), personal.into()),
    ]));
    config
}

/// Send one request through `provider` and collect what it yields.
async fn request(provider: &Openai) -> Vec<Event> {
    // Served without streaming, so the stand-in answers with one JSON body.
    let mut model = ModelDetails::empty("openai/gpt-6.2-sol".parse().unwrap());
    model.features = vec![STREAMING_UNSUPPORTED];

    let query = ChatQuery::from(Thread {
        system_prompt: None,
        sections: vec![],
        attachments: vec![],
        events: ConversationStream::new_test().with_turn("hello"),
    });

    provider
        .chat_completion_stream(&model, query)
        .await
        .unwrap()
        .map(|event| event.expect("the stand-in admits every request in the end"))
        .collect()
        .await
}

/// The API withholds the same items on every request that replays the edited
/// prefix, so a turn's tool rounds would each repeat the notice.
#[tokio::test]
async fn test_a_repeated_removal_is_reported_once_per_provider() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = AppConfig::new_test().providers.llm.openai;
    config.base_url = format!("http://{}", listener.local_addr().unwrap());
    let server = serve(listener, |_, _| completed(&["rs_1"]));
    let provider = Openai::with_credential(&config, Credential::ApiKey("key".to_owned()));

    let first = request(&provider).await;
    let second = request(&provider).await;

    // A provider serves one turn, so the next turn's provider reports again.
    let next_turn = Openai::with_credential(&config, Credential::ApiKey("key".to_owned()));
    let third = request(&next_turn).await;
    server.abort();

    assert_eq!(first, vec![
        Event::Notice(
            "OpenAI dropped 1 reasoning item from earlier turns because the conversation before \
             them changed (prefix_mismatch); the model may redo that reasoning."
                .to_owned()
        ),
        Event::Finished(FinishReason::Completed),
    ]);
    assert_eq!(second, vec![Event::Finished(FinishReason::Completed)]);
    assert_eq!(third, vec![
        Event::Notice(
            "OpenAI dropped 1 reasoning item from earlier turns because the conversation before \
             them changed (prefix_mismatch); the model may redo that reasoning."
                .to_owned()
        ),
        Event::Finished(FinishReason::Completed),
    ]);
}

/// A later request that loses more reasoning reports what is new, counting only
/// the items no earlier request reported.
#[tokio::test]
async fn test_only_newly_removed_reasoning_is_reported() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = AppConfig::new_test().providers.llm.openai;
    config.base_url = format!("http://{}", listener.local_addr().unwrap());
    let server = serve(listener, |_, index| {
        if index == 0 {
            completed(&["rs_1", "rs_2"])
        } else {
            completed(&["rs_1", "rs_2", "rs_3"])
        }
    });
    let provider = Openai::with_credential(&config, Credential::ApiKey("key".to_owned()));

    let first = request(&provider).await;
    let second = request(&provider).await;
    server.abort();

    assert_eq!(first, vec![
        Event::Notice(
            "OpenAI dropped 2 reasoning items from earlier turns because the conversation before \
             them changed (prefix_mismatch); the model may redo that reasoning."
                .to_owned()
        ),
        Event::Finished(FinishReason::Completed),
    ]);
    assert_eq!(second, vec![
        Event::Notice(
            "OpenAI dropped 1 reasoning item from earlier turns because the conversation before \
             them changed (prefix_mismatch); the model may redo that reasoning."
                .to_owned()
        ),
        Event::Finished(FinishReason::Completed),
    ]);
}

/// Every request resolves the chain again and skips the same entry for the same
/// reason.
#[tokio::test]
async fn test_a_skipped_credential_is_reported_once_per_provider() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = two_key_config(&listener, UNSET_KEY_ENV, PERSONAL_KEY_ENV);
    let server = serve(listener, |_, _| completed(&[]));
    let provider = Openai::new(&config).unwrap();

    let first = request(&provider).await;
    let second = request(&provider).await;
    server.abort();

    assert_eq!(first, vec![
        Event::Notice(
            "skipping api_key:work: environment variable JP_TEST_OPENAI_NOTICE_KEY_UNSET is not \
             set"
            .to_owned()
        ),
        Event::Finished(FinishReason::Completed),
    ]);
    assert_eq!(second, vec![Event::Finished(FinishReason::Completed)]);
}

/// An API key the host refuses leaves no record behind, so every request tries
/// it again and switches again.
/// Each switch is an event and is reported; the skip that follows from it
/// restates the first one.
#[tokio::test]
async fn test_a_credential_switch_is_reported_every_time() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = two_key_config(&listener, WORK_KEY_ENV, PERSONAL_KEY_ENV);
    let refused = format!(
        "authorization: Bearer {}",
        std::env::var(WORK_KEY_ENV).unwrap()
    );
    let server = serve(listener, move |head, _| {
        if head.lines().any(|line| line.eq_ignore_ascii_case(&refused)) {
            unauthorized()
        } else {
            completed(&[])
        }
    });
    let provider = Openai::new(&config).unwrap();

    let first = request(&provider).await;
    let second = request(&provider).await;
    server.abort();

    assert_eq!(first, vec![
        Event::Notice("skipping api_key:work: already tried for this request".to_owned()),
        Event::Notice("api key (work) rejected, continuing with api key (personal)".to_owned()),
        Event::Finished(FinishReason::Completed),
    ]);
    assert_eq!(second, vec![
        Event::Notice("api key (work) rejected, continuing with api key (personal)".to_owned()),
        Event::Finished(FinishReason::Completed),
    ]);
}
