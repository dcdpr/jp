//! The host's end of a plugin session, for tests that play the host while the
//! plugin runs: what the plugin writes, the answers the test writes back, and a
//! client for the HTTP API `jp review serve` answers.

use std::{
    future::Future,
    io::{self, PipeWriter, Read, Write},
    net::{SocketAddr, TcpStream},
    sync::{Arc, Mutex},
    time::Duration,
};

use jp_plugin::message::{
    CreatedResponse, HostToPlugin, InterruptRequest, PluginToHost, QueryCompleteResponse,
    QueryOutcome, QueryRequest,
};
use serde_json::Value;
use tokio::time;

/// The conversation the host creates for a review's first pass.
pub(crate) const CONVERSATION: &str = "jp-c17629384710";

/// What the plugin wrote to the host.
#[derive(Clone, Default)]
pub(crate) struct Written(Arc<Mutex<Vec<u8>>>);

impl Write for Written {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Written {
    pub(crate) fn messages(&self) -> Vec<PluginToHost> {
        String::from_utf8(self.0.lock().unwrap().clone())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    pub(crate) fn queries(&self) -> Vec<QueryRequest> {
        self.messages()
            .into_iter()
            .filter_map(|message| match message {
                PluginToHost::Query(query) => Some(query),
                _ => None,
            })
            .collect()
    }

    pub(crate) fn interrupts(&self) -> Vec<InterruptRequest> {
        self.messages()
            .into_iter()
            .filter_map(|message| match message {
                PluginToHost::Interrupt(interrupt) => Some(interrupt),
                _ => None,
            })
            .collect()
    }

    /// The first query the plugin sent, once it has.
    pub(crate) async fn first_query(&self) -> QueryRequest {
        let written = self.clone();
        eventually(move || !written.queries().is_empty()).await;

        self.queries().remove(0)
    }
}

/// The host's end of the connection, which the test writes the host's answers
/// to.
pub(crate) struct Host(pub(crate) Option<PipeWriter>);

impl Host {
    pub(crate) fn answer(&mut self, message: &HostToPlugin) {
        let host = self.0.as_mut().unwrap();
        writeln!(host, "{}", serde_json::to_string(message).unwrap()).unwrap();
    }

    pub(crate) fn created(&mut self, query: &QueryRequest) {
        self.answer(&HostToPlugin::Created(CreatedResponse {
            id: query.id.clone(),
            conversation: CONVERSATION.to_owned(),
        }));
    }

    pub(crate) fn completed(&mut self, query: &QueryRequest, outcome: QueryOutcome) {
        self.answer(&HostToPlugin::QueryComplete(QueryCompleteResponse {
            id: query.id.clone(),
            conversation: CONVERSATION.to_owned(),
            outcome,
            data: None,
            answer: None,
        }));
    }

    /// Close the connection, as a host that went away does.
    pub(crate) fn leave(&mut self) {
        drop(self.0.take());
    }
}

/// Wait for `condition` to hold, failing the test after 5 seconds.
pub(crate) async fn eventually(condition: impl Fn() -> bool) {
    time::timeout(Duration::from_secs(5), async {
        while !condition() {
            time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("the condition didn't hold within 5 seconds");
}

/// Wait for `future`, failing the test after 5 seconds.
pub(crate) async fn within<T>(future: impl Future<Output = T>) -> T {
    time::timeout(Duration::from_secs(5), future)
        .await
        .expect("nothing within 5 seconds")
}

/// Send one HTTP request to the server on `address`, returning its status and
/// its body read as JSON.
///
/// The request names `address` as its `Host`, unless `headers` name another.
/// Blocks until the server answers and closes the connection.
pub(crate) fn http(
    address: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (u16, Value) {
    let (status, _, body) = http_text(address, method, path, headers, body);

    (status, serde_json::from_str(&body).unwrap())
}

/// [`http`], returning the answer's status, `Content-Type`, and body as they
/// are.
pub(crate) fn http_text(
    address: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (u16, String, String) {
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let host = if headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("host"))
    {
        String::new()
    } else {
        format!("Host: {address}\r\n")
    };
    let headers: String = headers
        .iter()
        .map(|(name, value)| [*name, ": ", *value, "\r\n"].concat())
        .collect();
    let request = format!(
        "{method} {path} HTTP/1.1\r\n{host}Connection: close\r\nContent-Length: \
         {}\r\n{headers}\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).unwrap();

    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let (head, body) = response.split_once("\r\n\r\n").unwrap();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    let content_type = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-type")
                .then(|| value.trim().to_owned())
        })
        .unwrap_or_default();

    (status, content_type, body.to_owned())
}
