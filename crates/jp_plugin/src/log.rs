//! Logging to the host.
//!
//! A plugin reports what happens as `log` messages: `jp` writes every one to
//! its trace file, and prints those at the verbosity the user asked for (`-v`).
//! A plugin sends every level, and leaves the choice to the host.
//!
//! [`Log`] is how a plugin logs: a message naming the event, the same every
//! time it happens, and what varies as JSON fields ([`fields`]).
//! A [`Client`] logs through the connection it holds, so a test that gives the
//! client a writer reads back what was logged; code that takes a `Log` can be
//! handed one that records instead.
//!
//! Code with no `Log` at hand can log through `tracing` instead: [`install`]
//! sets a [`LogLayer`] as the process's subscriber, which forwards one crate's
//! events, and [`LogHandle::activate`] gives the layer the writer it sends them
//! through.

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

/// How severe an event is, which the host chooses what to print by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Level(&'static str);

impl Level {
    pub const TRACE: Self = Self("trace");
    pub const DEBUG: Self = Self("debug");
    pub const INFO: Self = Self("info");
    pub const WARN: Self = Self("warn");
    pub const ERROR: Self = Self("error");

    /// The level as a `log` message names it, such as `info`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

/// Where a plugin's log events go.
pub trait Log {
    /// Log the event `message` at `level`, with `fields`.
    ///
    /// The message names the event and is the same every time it happens; what
    /// varies goes in `fields`.
    fn log_with(&self, level: Level, message: &'static str, fields: Map<String, Value>);

    /// Log the event `message` at `level`, with no fields.
    fn log(&self, level: Level, message: &'static str) {
        self.log_with(level, message, Map::new());
    }
}

/// An event's fields, from `(name, value)` pairs.
#[must_use]
pub fn fields<const N: usize>(pairs: [(&str, Value); N]) -> Map<String, Value> {
    pairs
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value))
        .collect()
}

/// A client logs by sending each event to the host; an event that can't be
/// written is lost, since a host that is gone has nowhere to put it.
impl Log for Client {
    fn log_with(&self, level: Level, message: &'static str, fields: Map<String, Value>) {
        drop(self.send(&PluginToHost::Log(LogMessage {
            level: level.as_str().to_owned(),
            message: message.to_owned(),
            fields,
        })));
    }
}

/// Set a [`LogLayer`] forwarding the events of the crate `target`, such as
/// `jp_serve_web`, as the process's subscriber, and return the handle that
/// activates it.
///
/// A process that has a subscriber already keeps it, and the handle activates a
/// layer nothing sends events to.
#[must_use]
pub fn install(target: &'static str) -> LogHandle {
    let (layer, handle) = LogLayer::new(target);
    drop(tracing_subscriber::registry().with(layer).try_init());

    handle
}

/// A `tracing` layer sending the events of one crate to the host as `log`
/// messages, every level of them, its fields included.
pub struct LogLayer {
    sink: &'static Mutex<Sink>,
    target: &'static str,
}

/// What activates a [`LogLayer`] once the writer to the host is known.
pub struct LogHandle {
    sink: &'static Mutex<Sink>,
}

/// Where events go: held until the layer is activated, then written.
enum Sink {
    Holding(Vec<LogMessage>),
    Writing(Writer),
}

impl LogLayer {
    /// A layer forwarding the events of the crate `target`, holding them until
    /// the handle it returns activates it.
    ///
    /// The layer and the handle share a sink that lives as long as the process:
    /// the layer is the process's subscriber for its whole life.
    #[must_use]
    pub fn new(target: &'static str) -> (Self, LogHandle) {
        let sink: &'static Mutex<Sink> = Box::leak(Box::new(Mutex::new(Sink::Holding(vec![]))));

        (Self { sink, target }, LogHandle { sink })
    }

    /// Whether an event of `target`, a module path, is forwarded: the crate's
    /// own, or one of its modules.
    fn forwards(&self, target: &str) -> bool {
        target
            .strip_prefix(self.target)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with("::"))
    }
}

impl LogHandle {
    /// Write every event held so far, and every later one, through `writer`.
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

        // The sink's lock is let go before writing, so an event raised while
        // another thread writes waits for the writer alone.
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

/// Write `message` to the host as one line; a host that is gone loses it.
fn write(writer: &mut dyn Write, message: &LogMessage) {
    let Ok(json) = serde_json::to_string(&PluginToHost::Log(message.clone())) else {
        return;
    };
    drop(writeln!(writer, "{json}"));
    drop(writer.flush());
}

/// An event's message and its other fields.
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

/// Acquire `mutex`, recovering the guard if a holder panicked: a log line is
/// not worth taking the plugin down for.
fn lock<T: ?Sized>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
#[path = "log_tests.rs"]
mod tests;
