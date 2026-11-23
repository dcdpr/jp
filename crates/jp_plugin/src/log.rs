//! Structured logging from command plugins to the `jp` host.
//!
//! Plugins emit structured [`LogMessage`] events over stdout; `jp` writes all
//! log levels to its trace file and prints events matching the CLI verbosity
//! (`-v`).
//!
//! [`Log`] is the logging trait: each call takes a static event message and
//! structured JSON [`fields`].
//! [`Client`] implements [`Log`] by writing [`PluginToHost::Log`] messages over
//! its shared stdout [`Writer`].
//!
//! Crates that log via `tracing` can call [`install`] to register a
//! [`LogLayer`] as the global subscriber for a target crate prefix, then call
//! [`LogHandle::activate`] once the host stdout [`Writer`] is ready.

use std::{
    fmt,
    io::Write,
    sync::{Mutex, MutexGuard, PoisonError},
};

use serde_json::{Map, Number, Value};
use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::{Layer, layer::Context, prelude::*};

use crate::{
    client::{Client, Writer},
    message::{LogMessage, PluginToHost},
};

/// Log severity level sent in [`LogMessage::level`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Level(&'static str);

impl Level {
    pub const TRACE: Self = Self("trace");
    pub const DEBUG: Self = Self("debug");
    pub const INFO: Self = Self("info");
    pub const WARN: Self = Self("warn");
    pub const ERROR: Self = Self("error");

    /// Return the wire name of the level (e.g. `"info"`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

/// Sink for structured plugin log events.
pub trait Log {
    /// Emit a log event at `level` with a static `message` and structured
    /// `fields`.
    fn log_with(&self, level: Level, message: &'static str, fields: Map<String, Value>);

    /// Emit a log event at `level` with no structured fields.
    fn log(&self, level: Level, message: &'static str) {
        self.log_with(level, message, Map::new());
    }
}

/// Build a JSON field map from `(name, value)` pairs.
#[must_use]
pub fn fields<const N: usize>(pairs: [(&str, Value); N]) -> Map<String, Value> {
    pairs
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value))
        .collect()
}

/// Forward log events to the host as [`PluginToHost::Log`] messages, ignoring
/// write errors if the host pipe has closed.
impl Log for Client {
    fn log_with(&self, level: Level, message: &'static str, fields: Map<String, Value>) {
        drop(self.send(&PluginToHost::Log(LogMessage {
            level: level.as_str().to_owned(),
            message: message.to_owned(),
            fields,
        })));
    }
}

/// Install a [`LogLayer`] as the global `tracing` subscriber for events under
/// `target` (e.g. `"jp_serve_web"`), returning the [`LogHandle`] that activates
/// output.
///
/// If a global subscriber is already installed, the existing subscriber is kept
/// and the returned handle is a no-op.
#[must_use]
pub fn install(target: &'static str) -> LogHandle {
    let (layer, handle) = LogLayer::new(target);
    drop(tracing_subscriber::registry().with(layer).try_init());

    handle
}

/// `tracing` layer that forwards events under a crate target prefix to the host
/// as [`LogMessage`]s.
pub struct LogLayer {
    sink: &'static Mutex<Sink>,
    target: &'static str,
}

/// Handle that attaches a [`Writer`] to a [`LogLayer`] and flushes buffered
/// pre-activation events.
pub struct LogHandle {
    sink: &'static Mutex<Sink>,
}

/// Buffered events before activation, or the active stdout [`Writer`].
enum Sink {
    Holding(Vec<LogMessage>),
    Writing(Writer),
}

impl LogLayer {
    /// Create a [`LogLayer`] for events under `target`, buffering events until
    /// [`LogHandle::activate`] is called.
    #[must_use]
    pub fn new(target: &'static str) -> (Self, LogHandle) {
        let sink: &'static Mutex<Sink> = Box::leak(Box::new(Mutex::new(Sink::Holding(vec![]))));

        (Self { sink, target }, LogHandle { sink })
    }

    /// Return whether `target` equals `self.target` or is a submodule path
    /// (`{self.target}::...`).
    fn forwards(&self, target: &str) -> bool {
        target
            .strip_prefix(self.target)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with("::"))
    }
}

impl LogHandle {
    /// Flush any buffered events to `writer` and route subsequent events
    /// through it.
    pub fn activate(&self, writer: &Writer) {
        let held = match std::mem::replace(&mut *lock(self.sink), Sink::Writing(writer.clone())) {
            Sink::Holding(held) => held,
            Sink::Writing(_) => return,
        };

        let mut writer = lock(writer);
        for message in &held {
            write(&mut *writer, message);
        }
    }
}

impl<S: Subscriber> Layer<S> for LogLayer {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        let meta = event.metadata();
        if !self.forwards(meta.target()) {
            return;
        }

        let mut fields = Fields::default();
        event.record(&mut fields);
        fields
            .map
            .insert("target".to_owned(), meta.target().to_owned().into());
        let message = LogMessage {
            level: meta.level().as_str().to_ascii_lowercase(),
            message: fields.message,
            fields: fields.map,
        };

        // Release the sink lock before acquiring the writer lock so concurrent
        // events only contend on the writer.
        let writer = match &mut *lock(self.sink) {
            Sink::Holding(held) => {
                held.push(message);
                return;
            }
            Sink::Writing(writer) => writer.clone(),
        };
        write(&mut *lock(&writer), &message);
    }
}

/// Write `message` to `writer` as a JSON line, ignoring errors if the pipe is
/// closed.
fn write(writer: &mut dyn Write, message: &LogMessage) {
    let Ok(json) = serde_json::to_string(&PluginToHost::Log(message.clone())) else {
        return;
    };
    drop(writeln!(writer, "{json}"));
    drop(writer.flush());
}

/// Visitor that extracts an event's `message` and structured fields.
#[derive(Default)]
struct Fields {
    message: String,
    map: Map<String, Value>,
}

impl Fields {
    fn put(&mut self, field: &Field, value: Value) {
        self.map.insert(field.name().to_owned(), value);
    }
}

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            value.clone_into(&mut self.message);
        } else {
            self.put(field, value.into());
        }
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.put(field, value.into());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.put(field, value.into());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.put(field, value.into());
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.put(
            field,
            Number::from_f64(value).map_or_else(|| value.to_string().into(), Value::Number),
        );
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        let text = format!("{value:?}");
        if field.name() == "message" {
            self.message = text;
        } else {
            self.put(field, text.into());
        }
    }
}

/// Lock `mutex`, recovering the inner value if a previous holder panicked.
fn lock<T: ?Sized>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
#[path = "log_tests.rs"]
mod tests;
