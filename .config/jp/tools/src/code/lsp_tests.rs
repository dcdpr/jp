use std::io::{self, Cursor};

use serde_json::json;

use super::{super::rust_analyzer::READY, *};

/// A writer the test can read back after the connection wrote to it.
#[derive(Clone, Default)]
struct Written(Arc<Mutex<Vec<u8>>>);

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
    fn messages(&self) -> Vec<Value> {
        let bytes = self.0.lock().unwrap().clone();
        let mut reader = Cursor::new(bytes);
        let mut messages = vec![];
        while let Some(message) = read_message(&mut reader).unwrap() {
            messages.push(message);
        }
        messages
    }
}

fn frames(messages: &[Value]) -> Cursor<Vec<u8>> {
    let mut bytes = vec![];
    for message in messages {
        write_message(&mut bytes, message).unwrap();
    }
    Cursor::new(bytes)
}

fn status(quiescent: bool, health: &str, message: Option<&str>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "experimental/serverStatus",
        "params": { "health": health, "quiescent": quiescent, "message": message },
    })
}

#[test]
fn writes_a_content_length_frame() {
    let mut bytes = vec![];
    write_message(&mut bytes, &json!({ "id": 1 })).unwrap();

    assert_eq!(
        String::from_utf8(bytes).unwrap(),
        "Content-Length: 8\r\n\r\n{\"id\":1}"
    );
}

#[test]
fn reads_frames_until_the_stream_ends() {
    let mut reader = Cursor::new(
        "Content-Length: 8\r\nContent-Type: \
         application/vscode-jsonrpc\r\n\r\n{\"id\":1}content-length: 8\r\n\r\n{\"id\":2}",
    );

    assert_eq!(read_message(&mut reader).unwrap(), Some(json!({ "id": 1 })));
    assert_eq!(read_message(&mut reader).unwrap(), Some(json!({ "id": 2 })));
    assert_eq!(read_message(&mut reader).unwrap(), None);
}

#[test]
fn a_stream_ending_inside_a_header_is_an_error() {
    let mut reader = Cursor::new("Content-Length: 8\r\n");

    let error = read_message(&mut reader).unwrap_err();

    assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
}

#[test]
fn a_request_returns_the_result_with_its_id() {
    let reader = frames(&[
        json!({ "jsonrpc": "2.0", "id": 7, "result": "stale" }),
        json!({ "jsonrpc": "2.0", "method": "$/progress", "params": {} }),
        json!({ "jsonrpc": "2.0", "id": 1, "result": { "answer": 42 } }),
    ]);
    let mut connection = Connection::new(reader, Written::default());

    let result = connection
        .request("test/ask", &json!({}), Duration::from_secs(5))
        .unwrap();

    assert_eq!(result, json!({ "answer": 42 }));
}

#[test]
fn an_error_response_names_the_method_and_code() {
    let reader = frames(&[json!({
        "jsonrpc": "2.0",
        "id": 1,
        "error": { "code": -32801, "message": "content modified" },
    })]);
    let mut connection = Connection::new(reader, Written::default());

    let error = connection
        .request(
            "textDocument/references",
            &json!({}),
            Duration::from_secs(5),
        )
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "`textDocument/references` failed (-32801): content modified"
    );
}

#[test]
fn server_requests_are_answered_with_null() {
    let reader = frames(&[
        json!({ "jsonrpc": "2.0", "id": "create-1", "method": "window/workDoneProgress/create", "params": {} }),
        json!({ "jsonrpc": "2.0", "id": 1, "result": null }),
    ]);
    let written = Written::default();
    let mut connection = Connection::new(reader, written.clone());

    connection
        .request("test/ask", &json!({ "q": 1 }), Duration::from_secs(5))
        .unwrap();

    // The reader thread answers concurrently with the request being sent, so
    // the two messages can be written in either order.
    let mut messages = written.messages();
    messages.sort_by_key(ToString::to_string);
    assert_eq!(messages, vec![
        json!({ "jsonrpc": "2.0", "id": "create-1", "result": null }),
        json!({ "jsonrpc": "2.0", "id": 1, "method": "test/ask", "params": { "q": 1 } }),
    ]);
}

#[test]
fn waits_until_the_server_is_quiescent() {
    let reader = frames(&[
        status(false, "ok", None),
        status(true, "warning", Some("proc macros failed to build")),
    ]);
    let mut connection = Connection::new(reader, Written::default());

    connection
        .wait_until_ready(READY, Duration::from_secs(5))
        .unwrap();

    // The wait went past the first status, which said the server was still
    // loading, to the second.
    assert_eq!(
        connection.latest.get("experimental/serverStatus"),
        Some(&json!({
            "health": "warning",
            "quiescent": true,
            "message": "proc macros failed to build",
        }))
    );
}

#[test]
fn a_quiescent_server_in_error_health_is_not_ready() {
    let reader = frames(&[status(true, "error", Some("cargo metadata failed"))]);
    let mut connection = Connection::new(reader, Written::default());

    let error = connection
        .wait_until_ready(READY, Duration::from_secs(5))
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "The language server could not load the workspace: cargo metadata failed"
    );
}

#[test]
fn a_closed_stream_ends_a_request() {
    let mut connection = Connection::new(Cursor::new(vec![]), Written::default());

    let error = connection
        .request("test/ask", &json!({}), Duration::from_secs(5))
        .unwrap_err();

    assert_eq!(
        error.to_string(),
        "The language server stopped: the server closed its output"
    );
}

#[test]
fn a_silent_server_times_out() {
    // The write end stays open, so the reader blocks instead of seeing the end
    // of the stream.
    let (reader, _writer) = io::pipe().unwrap();
    let mut connection = Connection::new(BufReader::new(reader), Written::default());

    let error = connection
        .request("test/ask", &json!({}), Duration::from_millis(50))
        .unwrap_err();

    assert!(
        matches!(&error, LspError::Timeout { method, .. } if method == "test/ask"),
        "{error}"
    );
}

#[test]
fn characters_count_in_the_negotiated_encoding() {
    // `é` is two bytes in UTF-8 and one unit in UTF-16; `𝄞` is four bytes and
    // two units.
    let line = "é𝄞 iter";
    let byte = line.find("iter").unwrap();

    assert_eq!(PositionEncoding::Utf8.character(line, byte), 7);
    assert_eq!(PositionEncoding::Utf16.character(line, byte), 4);
}
