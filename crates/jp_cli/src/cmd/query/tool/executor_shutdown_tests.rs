use reqwest::{Client, Error as HttpError, Response};
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

use super::{RoleClient, fixture};

struct EmptyServer;
impl ServerHandler for EmptyServer {}

// rmcp logs HTTP deletion failures without returning them from cancel(). This
// close callback reports the actual DELETE result to the test instead.
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
        let result = Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .delete(&self.url)
            .header("mcp-session-id", &self.session)
            .header("mcp-protocol-version", "2025-11-25")
            .send()
            .await
            .and_then(Response::error_for_status)
            .map(|_| ());
        self.result.take().unwrap().send(result).unwrap();
        self.inner.close().await
    }
}

#[tokio::test]
async fn client_can_delete_its_session_before_the_listener_stops() {
    timeout(Duration::from_secs(5), async {
        let (_source, mut owner, _executor, _count) = fixture("unattended").await;
        owner.client.take().unwrap().cancel().await.unwrap();
        let url = owner.endpoint.as_ref().unwrap().url().to_owned();
        let initialized = Client::builder().no_proxy().build().unwrap().post(&url)
            .header("accept", "application/json, text/event-stream")
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"cleanup-test","version":"1"}}}))
            .send().await.unwrap().error_for_status().unwrap();
        let session = initialized.headers()["mcp-session-id"].to_str().unwrap().to_owned();
        initialized.bytes().await.unwrap();

        let (client_io, server_io) = io::duplex(4096);
        let remote = tokio::spawn(async { EmptyServer.serve(server_io).await.unwrap().waiting().await.unwrap() });
        let (read, write) = io::split(client_io);
        let (result, deleted) = oneshot::channel();
        owner.client = Some(().serve(DeleteOnClose {
            inner: AsyncRwTransport::new_client(read, write), url, session, result: Some(result),
        }).await.unwrap());
        owner.shutdown().await.unwrap();
        deleted.await.unwrap().unwrap();
        remote.await.unwrap();
    }).await.unwrap();
}
