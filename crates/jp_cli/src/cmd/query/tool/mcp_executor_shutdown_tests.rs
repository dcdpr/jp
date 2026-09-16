//! What a client is still allowed to do while the endpoint is shutting down.

use reqwest::{Client as HttpClient, Error as HttpError, Response};
use rmcp::{
    ServerHandler, ServiceExt as _,
    model::{ClientJsonRpcMessage, ServerJsonRpcMessage},
    transport::{Transport, async_rw::AsyncRwTransport},
};
use serde_json::json;
use tokio::{
    io,
    sync::oneshot,
    time::{Duration, timeout},
};

use super::{Fixture, RoleClient};

/// An MCP server with no tools, standing in for the one a real client talks to.
struct EmptyServer;
impl ServerHandler for EmptyServer {}

/// Reports what a session-closing DELETE actually returned.
///
/// rmcp logs an HTTP deletion failure and returns `Ok` from `cancel()` anyway,
/// so a test watching only the return value cannot tell a served DELETE from a
/// refused one.
struct DeleteOnClose<T> {
    inner: T,
    url: String,
    session: String,
    result: Option<oneshot::Sender<Result<(), HttpError>>>,
}

impl<T: Transport<RoleClient>> Transport<RoleClient> for DeleteOnClose<T> {
    type Error = T::Error;

    fn send(
        &mut self,
        message: ClientJsonRpcMessage,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        self.inner.send(message)
    }

    async fn receive(&mut self) -> Option<ServerJsonRpcMessage> {
        self.inner.receive().await
    }

    async fn close(&mut self) -> Result<(), Self::Error> {
        let result = HttpClient::builder()
            .no_proxy()
            .build()
            .expect("an HTTP client with no proxy")
            .delete(&self.url)
            .header("mcp-session-id", &self.session)
            .header("mcp-protocol-version", "2025-11-25")
            .send()
            .await
            .and_then(Response::error_for_status)
            .map(drop);
        if let Some(sender) = self.result.take() {
            drop(sender.send(result));
        }

        self.inner.close().await
    }
}

/// Shutdown closes the MCP session before it stops the listener, and the DELETE
/// that closes it is an HTTP request the listener has to still be serving.
///
/// Stopping the listener first would leave the session open on a server that
/// can no longer hear about it.
#[tokio::test]
async fn a_client_can_close_its_session_before_the_listener_stops() {
    timeout(Duration::from_secs(5), async {
        let mut fixture = Fixture::inquiring("unattended").await;

        // Retire the fixture's own connection, leaving the endpoint running so
        // the session below is the only one outstanding.
        fixture
            .owner
            .client
            .take()
            .expect("the fixture connected a client")
            .cancel()
            .await
            .unwrap();
        let url = fixture
            .owner
            .endpoint
            .as_ref()
            .expect("the fixture started an endpoint")
            .url()
            .to_owned();

        let initialized = HttpClient::builder()
            .no_proxy()
            .build()
            .unwrap()
            .post(&url)
            .header("accept", "application/json, text/event-stream")
            .json(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "clientInfo": {"name": "shutdown-test", "version": "1"},
                },
            }))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        let session = initialized.headers()["mcp-session-id"]
            .to_str()
            .unwrap()
            .to_owned();
        initialized.bytes().await.unwrap();

        // The client's own transport is a local pipe: only the DELETE it sends
        // on close has to reach the endpoint.
        let (client_io, server_io) = io::duplex(4096);
        let remote = tokio::spawn(async {
            EmptyServer
                .serve(server_io)
                .await
                .unwrap()
                .waiting()
                .await
                .unwrap()
        });
        let (read, write) = io::split(client_io);
        let (result, deleted) = oneshot::channel();
        fixture.owner.client = Some(
            ().serve(DeleteOnClose {
                inner: AsyncRwTransport::new_client(read, write),
                url,
                session,
                result: Some(result),
            })
            .await
            .unwrap(),
        );

        fixture.owner.shutdown().await.unwrap();

        deleted
            .await
            .expect("the transport reported its DELETE")
            .expect("the listener served the session DELETE");
        remote.await.unwrap();
    })
    .await
    .unwrap();
}
