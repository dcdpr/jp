use std::{
    sync::{Arc, Mutex},
    thread,
};

use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

/// What was written to the host.
#[derive(Clone, Default)]
struct Written(Arc<Mutex<Vec<u8>>>);

impl Write for Written {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Written {
    fn writer(&self) -> Writer {
        Arc::new(Mutex::new(Box::new(self.clone())))
    }

    fn messages(&self) -> Vec<LogMessage> {
        String::from_utf8(self.0.lock().unwrap().clone())
            .unwrap()
            .lines()
            .map(|line| match serde_json::from_str(line).unwrap() {
                PluginToHost::Log(log) => log,
                other => panic!("not a log: {other:?}"),
            })
            .collect()
    }

    fn shown(&self) -> Vec<String> {
        self.messages()
            .into_iter()
            .map(|message| message.message)
            .collect()
    }
}

/// The data goes in fields, as JSON, so the host can filter on it.
#[test]
fn a_client_logs_through_its_connection() {
    let written = Written::default();
    let client = Client::new(written.writer());

    client.log_with(
        Level::WARN,
        "Failed to stop a pass.",
        fields([("pass", 2.into()), ("lookups", json!(["rfd", "ticket"]))]),
    );
    client.log(Level::DEBUG, "Stopped.");

    assert_eq!(written.messages(), [
        LogMessage {
            level: "warn".to_owned(),
            message: "Failed to stop a pass.".to_owned(),
            fields: json!({ "pass": 2, "lookups": ["rfd", "ticket"] })
                .as_object()
                .unwrap()
                .clone(),
        },
        LogMessage {
            level: "debug".to_owned(),
            message: "Stopped.".to_owned(),
            fields: Map::new(),
        },
    ]);
}

#[test]
fn each_level_is_named_the_way_the_host_reads_it() {
    assert_eq!(
        [
            Level::TRACE,
            Level::DEBUG,
            Level::INFO,
            Level::WARN,
            Level::ERROR
        ]
        .map(Level::as_str),
        ["trace", "debug", "info", "warn", "error"]
    );
}

/// An event's fields reach the host with their types, beside its target.
#[test]
fn a_tracing_event_is_written_with_its_fields() {
    let (layer, handle) = LogLayer::new("jp_review");
    let _subscriber = tracing::subscriber::set_default(tracing_subscriber::registry().with(layer));
    let written = Written::default();
    handle.activate(&written.writer());

    tracing::warn!(
        target: "jp_review::running",
        review = "R-1h4k2m9",
        pass = 2_u64,
        delta = -1_i64,
        stopped = true,
        error = %"no host",
        "Failed to stop a pass."
    );

    let messages = written.messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(
        (messages[0].level.as_str(), messages[0].message.as_str()),
        ("warn", "Failed to stop a pass.")
    );
    assert_eq!(
        Value::Object(messages[0].fields.clone()),
        json!({
            "target": "jp_review::running",
            "review": "R-1h4k2m9",
            "pass": 2,
            "delta": -1,
            "stopped": true,
            "error": "no host"
        })
    );
}

/// The host keeps every level in its trace file, whatever its terminal shows,
/// so nothing is left out here.
#[test]
fn every_level_is_sent_including_what_came_before_activation() {
    let (layer, handle) = LogLayer::new("jp_review");
    let _subscriber = tracing::subscriber::set_default(tracing_subscriber::registry().with(layer));

    tracing::trace!(target: "jp_review", "before");
    let written = Written::default();
    handle.activate(&written.writer());
    tracing::debug!(target: "jp_review", "after");

    assert_eq!(written.shown(), ["before", "after"]);
    assert_eq!(
        written
            .messages()
            .iter()
            .map(|message| message.level.as_str())
            .collect::<Vec<_>>(),
        ["trace", "debug"]
    );
}

/// A dependency's events, or another crate whose name starts the same way, are
/// not the plugin's to report.
#[test]
fn only_the_plugin_s_own_events_are_forwarded() {
    let (layer, handle) = LogLayer::new("jp_review");
    let _subscriber = tracing::subscriber::set_default(tracing_subscriber::registry().with(layer));
    let written = Written::default();
    handle.activate(&written.writer());

    tracing::info!(target: "hyper::proto", "dependency");
    tracing::info!(target: "jp_reviewer", "another crate");
    tracing::info!(target: "jp_review", "plugin");
    tracing::info!(target: "jp_review::server", "module");

    assert_eq!(written.shown(), ["plugin", "module"]);
}

/// A target is a module path: the crate's own name, or one of its modules.
#[test]
fn the_layer_forwards_the_crate_and_its_modules() {
    let (layer, _) = LogLayer::new("jp_review");

    assert!(layer.forwards("jp_review"));
    assert!(layer.forwards("jp_review::server"));
    assert!(!layer.forwards("jp_reviewer"));
    assert!(!layer.forwards("jp_plugin::client"));
}

/// The layer is the whole process's subscriber, so the event of a thread with
/// no subscriber of its own reaches the host.
#[test]
fn the_installed_layer_takes_every_thread_s_events() {
    let handle = install("jp_installed");
    let written = Written::default();
    handle.activate(&written.writer());

    thread::spawn(|| tracing::error!(target: "jp_installed", "from another thread"))
        .join()
        .unwrap();

    assert_eq!(written.shown(), ["from another thread"]);
}
