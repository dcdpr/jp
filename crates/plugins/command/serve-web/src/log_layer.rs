//! Custom tracing layer that sends log events through the plugin protocol.
//!
//! Events are buffered in memory until the protocol connection is ready,
//! then flushed and all subsequent events are written as `PluginToHost::Log`
//! JSON-lines on stdout.

use std::{io::Write, sync::Mutex};

use jp_plugin::message::{LogMessage, PluginToHost};
use tracing::{
    Event, Level, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::{Layer, layer::Context};

use crate::client::SharedWriter;

struct BufferedEvent {
    level: Level,
    target: String,
    message: String,
}

enum Sink {
    /// Events are held in memory until the protocol is ready.
    Buffering(Vec<BufferedEvent>),
    /// Events are written to stdout as protocol messages.
    Active {
        writer: SharedWriter,
        min_level: Level,
    },
}

/// A tracing [`Layer`] that routes events through the plugin protocol.
pub struct ProtocolLogLayer {
    sink: &'static Mutex<Sink>,
}

/// Handle used to activate the layer once the protocol client is ready.
pub struct ProtocolLogHandle {
    sink: &'static Mutex<Sink>,
}

impl ProtocolLogLayer {
    /// Create a new layer and its activation handle.
    ///
    /// The layer buffers events until [`ProtocolLogHandle::activate`] is
    /// called. Both the layer and handle share a `'static` reference to the
    /// sink (leaked via `Box::leak`). This is intentional: the layer is
    /// installed as the global tracing subscriber and lives for the entire
    /// process.
    pub fn new() -> (Self, ProtocolLogHandle) {
        let sink: &'static Mutex<Sink> =
            Box::leak(Box::new(Mutex::new(Sink::Buffering(Vec::new()))));

        (Self { sink }, ProtocolLogHandle { sink })
    }
}

impl ProtocolLogHandle {
    /// Switch from buffering to protocol mode.
    ///
    /// Flushes any buffered events that meet `min_level` through `writer`,
    /// then all future events are sent directly.
    pub fn activate(&self, writer: &SharedWriter, min_level: Level) {
        let buffer = {
            let mut sink = self.sink.lock().expect("sink lock");
            match std::mem::replace(&mut *sink, Sink::Active {
                writer: writer.clone(),
                min_level,
            }) {
                Sink::Buffering(buf) => buf,
                Sink::Active { .. } => return,
            }
        };

        // Flush buffered events outside the sink lock.
        if let Ok(mut w) = writer.lock() {
            for event in buffer {
                if event.level <= min_level {
                    write_log(&mut *w, event.level, &event.target, &event.message);
                }
            }
        }
    }
}

impl<S: Subscriber> Layer<S> for ProtocolLogLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();

        // Only capture events from our own crate.
        if !meta.target().starts_with("jp_serve_web") {
            return;
        }

        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);

        let mut sink = self.sink.lock().expect("sink lock");
        match &mut *sink {
            Sink::Buffering(buf) => {
                buf.push(BufferedEvent {
                    level: *meta.level(),
                    target: meta.target().to_owned(),
                    message: visitor.message,
                });
            }
            Sink::Active { writer, min_level } => {
                if meta.level() > min_level {
                    return;
                }
                // Use try_lock to avoid deadlocking when a tracing event
                // fires while the writer is already held (e.g. during a
                // protocol write in the same thread).
                if let Ok(mut w) = writer.try_lock() {
                    write_log(&mut *w, *meta.level(), meta.target(), &visitor.message);
                }
            }
        }
    }
}

fn write_log(w: &mut dyn Write, level: Level, target: &str, message: &str) {
    let level_str = match level {
        Level::TRACE => "trace",
        Level::DEBUG => "debug",
        Level::INFO => "info",
        Level::WARN => "warn",
        Level::ERROR => "error",
    };

    let mut fields = serde_json::Map::new();
    fields.insert(
        "target".to_owned(),
        serde_json::Value::String(target.to_owned()),
    );

    let msg = PluginToHost::Log(LogMessage {
        level: level_str.to_owned(),
        message: message.to_owned(),
        fields,
    });

    if let Ok(json) = serde_json::to_string(&msg) {
        drop(writeln!(w, "{json}"));
        drop(w.flush());
    }
}

/// Visitor that extracts the `message` field from a tracing event.
#[derive(Default)]
struct MessageVisitor {
    message: String,
}

impl Visit for MessageVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_owned();
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        }
    }
}

#[cfg(test)]
#[path = "log_layer_tests.rs"]
mod tests;
