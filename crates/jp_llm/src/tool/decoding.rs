//! Request-local instructions for decoding provider tool arguments.
//!
//! Providers attach a plan to a tool call's start.
//! The event builder applies it to the completed JSON object before
//! constructing the tool call request.

use std::{collections::BTreeMap, sync::Arc};

use futures::StreamExt as _;
use serde_json::{Map, Value};

use crate::{
    EventStream,
    event::{Event, EventPart, ToolCallPart},
};

/// Removes null placeholders introduced by a provider's schema encoding.
///
/// This is transient request state, not part of a persisted tool call.
/// Fields not named by the plan retain their values, including explicit nulls.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ArgumentDecoding {
    pub(crate) omit_null: Vec<String>,
    pub(crate) properties: BTreeMap<String, Self>,
    pub(crate) items: Option<Box<Self>>,
}

impl ArgumentDecoding {
    pub(crate) fn is_empty(&self) -> bool {
        self.omit_null.is_empty() && self.properties.is_empty() && self.items.is_none()
    }

    pub(crate) fn apply(&self, arguments: &mut Map<String, Value>) {
        for name in &self.omit_null {
            if arguments.get(name) == Some(&Value::Null) {
                arguments.remove(name);
            }
        }
        for (name, plan) in &self.properties {
            if let Some(value) = arguments.get_mut(name) {
                plan.apply_value(value);
            }
        }
    }

    fn apply_value(&self, value: &mut Value) {
        match value {
            Value::Object(arguments) => self.apply(arguments),
            Value::Array(values) => {
                if let Some(items) = &self.items {
                    for value in values {
                        items.apply_value(value);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Decoding plans for the tools advertised in one provider request.
#[derive(Debug, Default)]
pub(crate) struct ArgumentDecoders(BTreeMap<String, Arc<ArgumentDecoding>>);

impl ArgumentDecoders {
    pub(crate) fn insert(&mut self, name: &str, plan: ArgumentDecoding) {
        if !plan.is_empty() {
            self.0.insert(name.to_owned(), Arc::new(plan));
        }
    }

    /// Attach plans without buffering or changing generated argument chunks.
    pub(crate) fn attach(self, stream: EventStream) -> EventStream {
        stream
            .map(move |mut event| {
                if let Ok(Event::Part {
                    part: EventPart::ToolCall(ToolCallPart::Start { name, decoding, .. }),
                    ..
                }) = &mut event
                {
                    *decoding = self.0.get(name).cloned();
                }
                event
            })
            .boxed()
    }
}

#[cfg(test)]
#[path = "decoding_tests.rs"]
mod tests;
