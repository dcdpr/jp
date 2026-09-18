//! Fan-out: one tool call carrying several independent operations.
//!
//! A tool with fan-out enabled is shown an envelope schema instead of its own:
//! an object holding a single [`FAN_OUT_KEY`] array whose elements each hold
//! one complete set of the tool's arguments.
//! The tool implementation is untouched, and still receives one operation's
//! arguments per invocation.
//!
//! This module owns both halves of that translation: [`envelope`] builds the
//! schema the provider sees, and [`expand`] takes a call's arguments back apart
//! into the operations to run.
//!
//! Result folding lives with the caller that collects the responses, not here.

use jp_config::conversation::tool::FanOut;
use serde_json::{Map, Value, json};

/// The envelope's only property: the array of operations to run.
pub const FAN_OUT_KEY: &str = "ops";

/// Sentence appended to a fan-out tool's description, telling the model how the
/// envelope relates to the per-operation documentation it already has.
///
/// The tool's own `examples` need no rewrite: each one already shows exactly
/// one operation, which is the shape of one element.
pub const FAN_OUT_DESCRIPTION: &str = "This tool accepts several operations in a single call. Put \
                                       each one in the `ops` array as its own complete object; \
                                       the documented parameters and examples describe one \
                                       element. Batch every operation you already know you need \
                                       into one call rather than issuing them one at a time.";

/// Build the envelope schema wrapping a tool's per-operation schema.
///
/// The result is always an object with one required array property, whatever
/// shape `operation` has.
#[must_use]
pub fn envelope(operation: &Value) -> Value {
    json!({
        "type": "object",
        "properties": {
            FAN_OUT_KEY: {
                "type": "array",
                "minItems": 1,
                "description": "The operations to perform. Each element is one complete set of \
                                this tool's arguments.",
                "items": operation.clone(),
            }
        },
        "required": [FAN_OUT_KEY],
        "additionalProperties": false,
    })
}

/// Why a call's arguments could not be taken apart into operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpandError {
    /// The `ops` key is absent.
    Missing,

    /// `ops` is present but is not an array.
    NotAnArray,

    /// `ops` is an array with nothing in it.
    Empty,

    /// An element of `ops` is not an object.
    ElementNotAnObject {
        /// Zero-based position of the offending element.
        index: usize,
    },
}

impl ExpandError {
    /// The message handed back to the assistant.
    ///
    /// Each one names the envelope explicitly, because the model reaching this
    /// point has the envelope schema in front of it and got the shape wrong.
    #[must_use]
    pub fn message(&self, tool_name: &str) -> String {
        match self {
            Self::Missing => format!(
                "Tool '{tool_name}' takes its arguments in an `{FAN_OUT_KEY}` array, but the call \
                 had no `{FAN_OUT_KEY}` key. Wrap the arguments in one: {{\"{FAN_OUT_KEY}\": \
                 [{{...}}]}}."
            ),
            Self::NotAnArray => format!(
                "Tool '{tool_name}' expects `{FAN_OUT_KEY}` to be an array of operations, and the \
                 call gave it something else."
            ),
            Self::Empty => format!(
                "Tool '{tool_name}' was called with an empty `{FAN_OUT_KEY}` array, so there was \
                 nothing to do. Include at least one operation."
            ),
            Self::ElementNotAnObject { index } => format!(
                "Tool '{tool_name}' expects every element of `{FAN_OUT_KEY}` to be an object \
                 holding one operation's arguments; element {index} was not."
            ),
        }
    }
}

/// Take a fan-out call's arguments apart into one argument map per operation.
///
/// The returned maps are what the tool is actually invoked with, so each is the
/// shape the tool's own schema describes.
///
/// A call that omits the envelope entirely but looks like a single operation is
/// **not** accepted: a tool whose schema says `ops` and receives `path` has
/// been called wrongly, and silently running it would hide the mistake from the
/// model that made it.
///
/// # Errors
///
/// Returns [`ExpandError`] when the envelope is absent, is not an array, is
/// empty, or holds a non-object element.
pub fn expand(arguments: &Map<String, Value>) -> Result<Vec<Map<String, Value>>, ExpandError> {
    let Some(value) = arguments.get(FAN_OUT_KEY) else {
        return Err(ExpandError::Missing);
    };

    let Some(items) = value.as_array() else {
        return Err(ExpandError::NotAnArray);
    };

    if items.is_empty() {
        return Err(ExpandError::Empty);
    }

    items
        .iter()
        .enumerate()
        .map(|(index, item)| match item {
            Value::Object(map) => Ok(map.clone()),
            _ => Err(ExpandError::ElementNotAnObject { index }),
        })
        .collect()
}

/// How one operation ended, for the folded result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationOutcome {
    /// The operation ran and succeeded.
    Ok(String),

    /// The operation ran and reported an error.
    Error(String),

    /// The operation never started, because an earlier one failed under
    /// `on_error = "stop"`.
    NotRun {
        /// One-based position of the operation whose failure stopped the rest.
        after: usize,
    },
}

/// Fold per-operation outcomes into the single body the assistant receives.
///
/// Each operation gets a header naming its position, so a model reading the
/// result can line each section up with the operation it wrote.
/// Operations that never started say so explicitly: without that, a model that
/// asked for five and reads three assumes the other two succeeded silently.
///
/// A single successful operation is returned bare, with no framing at all, so a
/// one-operation fan-out call reads exactly like a call to the same tool
/// without fan-out.
#[must_use]
pub fn fold(outcomes: &[OperationOutcome]) -> String {
    if let [OperationOutcome::Ok(content)] = outcomes {
        return content.clone();
    }

    let count = outcomes.len();
    let mut body = String::new();

    for (index, outcome) in outcomes.iter().enumerate() {
        if index > 0 {
            body.push('\n');
        }

        let position = index + 1;
        match outcome {
            OperationOutcome::Ok(content) => {
                body.push_str(&format!("[{position}/{count}] ok\n{content}\n"));
            }
            OperationOutcome::Error(message) => {
                body.push_str(&format!("[{position}/{count}] error\n{message}\n"));
            }
            OperationOutcome::NotRun { after } => {
                body.push_str(&format!(
                    "[{position}/{count}] not run (stopped after operation {after} failed)\n"
                ));
            }
        }
    }

    body
}

/// Whether the outcomes so far mean no further operation should start.
#[must_use]
pub fn should_stop(fan_out: FanOut, outcomes: &[OperationOutcome]) -> bool {
    fan_out.stops_on_error()
        && outcomes
            .iter()
            .any(|outcome| matches!(outcome, OperationOutcome::Error(_)))
}

#[cfg(test)]
#[path = "fan_out_tests.rs"]
mod tests;
