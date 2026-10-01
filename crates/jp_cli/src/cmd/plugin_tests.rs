use std::time::Duration;

use tokio::{sync::oneshot, time::timeout};

use super::*;

/// The request stands in for SIGTERM or Ctrl-C: the token is cancelled when it
/// arrives, and not before.
#[tokio::test]
async fn a_shutdown_request_cancels_the_token() {
    let token = CancellationToken::new();
    let (request, requested) = oneshot::channel::<()>();
    cancel_when(&token, async move {
        drop(requested.await);
    });

    tokio::task::yield_now().await;
    assert!(!token.is_cancelled(), "cancelled before any request");

    request.send(()).unwrap();
    timeout(Duration::from_secs(5), token.cancelled())
        .await
        .expect("cancelled once the request arrived");
}
