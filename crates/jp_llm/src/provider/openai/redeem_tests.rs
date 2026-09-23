//! Redeeming reset credits against a local stand-in for the account endpoints.

use std::sync::{Arc, Mutex};

use jp_config::AppConfig;
use serde_json::Value;
use tokio::{
    io::{AsyncReadExt as _, AsyncWriteExt as _},
    net::{TcpListener, TcpStream},
};

use super::{Openai, resolve};
use crate::credential::Credential;

/// Serve the credit listing and the redemption endpoint until the test ends,
/// recording each redemption body.
async fn serve(listener: TcpListener, redemptions: Arc<Mutex<Vec<Value>>>) {
    loop {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };

        let (target, body) = read_request(&mut socket).await;

        let reply = if target.ends_with("/wham/rate-limit-reset-credits") {
            r#"{ "credits": [], "available_count": 2 }"#
        } else if target.ends_with("/wham/rate-limit-reset-credits/consume") {
            redemptions
                .lock()
                .unwrap()
                .push(serde_json::from_str(&body).unwrap());
            r#"{ "code": "reset", "windows_reset": 1 }"#
        } else {
            "{}"
        };

        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: \
             {}\r\nConnection: close\r\n\r\n{reply}",
            reply.len()
        );
        socket.write_all(response.as_bytes()).await.unwrap();
    }
}

/// Read one request's target and body.
async fn read_request(socket: &mut TcpStream) -> (String, String) {
    let mut raw = vec![];
    let mut chunk = [0u8; 1024];

    loop {
        let read = socket.read(&mut chunk).await.unwrap();
        raw.extend_from_slice(&chunk[..read]);

        let text = String::from_utf8_lossy(&raw).to_string();
        let Some((head, body)) = text.split_once("\r\n\r\n") else {
            continue;
        };

        let length = head
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())?
            })
            .unwrap_or(0);

        if body.len() >= length || read == 0 {
            let target = head
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
                .to_owned();
            return (target, body.to_owned());
        }
    }
}

/// A conversation that runs out twice spends two credits, so its two
/// redemptions must not share an idempotency key.
#[tokio::test]
async fn test_two_redemptions_in_one_conversation_use_distinct_keys() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let redemptions = Arc::new(Mutex::new(vec![]));
    let server = tokio::spawn(serve(listener, redemptions.clone()));

    let mut config = AppConfig::new_test().providers.llm.openai;
    config.codex_base_url = format!("http://{address}/backend-api/codex");
    let provider = Openai::with_credential(&config, Credential::Bearer("token".to_owned()));
    let attempt = resolve::Attempt::injected(
        Credential::Bearer("token".to_owned()),
        resolve::Attribution::default(),
    );

    let first = provider.redeem_reset_credit(&attempt, "session-1").await;
    let second = provider.redeem_reset_credit(&attempt, "session-1").await;
    server.abort();

    assert_eq!(
        first.as_deref(),
        Some("subscription limit reached, redeemed a usage reset (1 left)")
    );
    assert!(second.is_some());

    let redemptions = redemptions.lock().unwrap();
    assert_eq!(redemptions.len(), 2);
    assert_ne!(
        redemptions[0]["redeem_request_id"],
        redemptions[1]["redeem_request_id"]
    );
    for body in redemptions.iter() {
        assert_ne!(body["redeem_request_id"], "session-1");
        assert!(body.get("credit_id").is_none(), "{body}");
    }
}
