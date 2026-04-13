use std::sync::{Arc, Mutex};

use tracing::Level;
use tracing_subscriber::prelude::*;

use super::*;

/// A writer that captures bytes for inspection.
#[derive(Default, Clone)]
struct TestWriter {
    buf: Arc<Mutex<Vec<u8>>>,
}

impl std::io::Write for TestWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.buf.lock().unwrap().extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl TestWriter {
    fn lines(&self) -> Vec<String> {
        let buf = self.buf.lock().unwrap();
        let text = String::from_utf8_lossy(&buf);
        text.lines().map(String::from).collect()
    }
}

fn make_shared(tw: &TestWriter) -> SharedWriter {
    Arc::new(Mutex::new(Box::new(tw.clone())))
}

#[test]
fn buffered_events_are_flushed_on_activate() {
    let (layer, handle) = ProtocolLogLayer::new();

    let _guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(layer));

    tracing::debug!(target: "jp_serve_web::routes", "request received");
    tracing::info!(target: "jp_serve_web::routes", "rendered page");

    let tw = TestWriter::default();
    handle.activate(&make_shared(&tw), Level::DEBUG);

    let lines = tw.lines();
    assert_eq!(lines.len(), 2);
    assert!(lines[0].contains("request received"));
    assert!(lines[1].contains("rendered page"));
}

#[test]
fn events_below_min_level_are_dropped() {
    let (layer, handle) = ProtocolLogLayer::new();

    let _guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(layer));

    tracing::trace!(target: "jp_serve_web::routes", "very verbose");
    tracing::debug!(target: "jp_serve_web::routes", "somewhat verbose");
    tracing::info!(target: "jp_serve_web::routes", "normal");

    // Activate at INFO — trace and debug should be dropped.
    let tw = TestWriter::default();
    handle.activate(&make_shared(&tw), Level::INFO);

    let lines = tw.lines();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("normal"));
}

#[test]
fn non_jp_serve_web_events_are_ignored() {
    let (layer, handle) = ProtocolLogLayer::new();

    let _guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(layer));

    tracing::info!(target: "tokio::runtime", "tokio noise");
    tracing::info!(target: "jp_serve_web::routes", "our event");

    let tw = TestWriter::default();
    handle.activate(&make_shared(&tw), Level::TRACE);

    let lines = tw.lines();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("our event"));
}

#[test]
fn active_events_sent_directly() {
    let (layer, handle) = ProtocolLogLayer::new();

    let _guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(layer));

    let tw = TestWriter::default();
    handle.activate(&make_shared(&tw), Level::DEBUG);

    tracing::debug!(target: "jp_serve_web::routes", "after activate");

    let lines = tw.lines();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("after activate"));

    // Verify it's valid protocol JSON.
    let msg: PluginToHost = serde_json::from_str(&lines[0]).unwrap();
    assert!(matches!(msg, PluginToHost::Log(_)));
}
