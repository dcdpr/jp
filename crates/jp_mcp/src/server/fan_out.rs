//! Fan-out: one tool call carrying several independent operations.
//!
//! A tool with `fan_out` configured is advertised with an envelope schema: an
//! object holding a single [`FAN_OUT_KEY`] array whose elements each hold one
//! complete set of the tool's own arguments.
//! The service expands a call carrying that envelope into one child invocation
//! per operation, so the tool itself still receives one operation's arguments
//! per invocation.
//!
//! This module holds the pure halves of that translation: [`advertise`] builds
//! what callers are shown, [`split`] decides whether a call's arguments are an
//! envelope or an ordinary call, and [`fold`] frames the operations' results
//! into the one response the caller receives.
//! Running the operations lives in the service.

use jp_tool::{ToolDefinition, ToolResult, schema::Node};
use serde_json::{Map, Value, json};

use super::service::validate_arguments;

/// The envelope's only property: the array of operations to run.
pub const FAN_OUT_KEY: &str = "ops";

/// Sentence appended to a fan-out tool's advertised description.
///
/// It asks for batching; the envelope schema already says what one element
/// holds.
pub const BATCH_DESCRIPTION: &str = "Batch every operation you already know you need into one \
                                     call, each as its own element of `ops`, rather than issuing \
                                     them one at a time.";

/// The definition a fan-out tool is advertised with.
///
/// The parameters are the [`envelope`] around the tool's own schema, and the
/// description gains [`BATCH_DESCRIPTION`].
/// The name is unchanged, and so is everything a caller never sees.
#[must_use]
pub fn advertise(definition: &ToolDefinition) -> ToolDefinition {
    let mut advertised = definition.clone();
    advertised.parameters = envelope(&definition.parameters);
    advertised.docs.summary = Some(match definition.docs.schema_description() {
        Some(description) => format!("{description} {BATCH_DESCRIPTION}"),
        None => BATCH_DESCRIPTION.to_owned(),
    });
    advertised
}

/// Build the envelope schema wrapping a tool's per-operation schema.
///
/// The result is always an object with one required array property, whatever
/// shape `operation` has.
///
/// A `$defs` or `definitions` block moves from the operation schema to the
/// envelope's root.
/// Same-document references are anchored at the document root (`#/$defs/Name`),
/// so leaving the block nested under `properties.ops.items` would point every
/// reference at a root that no longer holds it.
///
/// A schema referring to its own root (`$ref: "#"`) is not rewritten, and would
/// resolve to the envelope rather than the operation.
#[must_use]
pub fn envelope(operation: &Value) -> Value {
    let mut operation = operation.clone();
    let definitions = operation.as_object_mut().map(|object| {
        ["$defs", "definitions"]
            .into_iter()
            .filter_map(|key| object.remove(key).map(|block| (key.to_owned(), block)))
            .collect::<Vec<_>>()
    });

    let mut envelope = json!({
        "type": "object",
        "properties": {
            FAN_OUT_KEY: {
                "type": "array",
                "minItems": 1,
                "description": "The operations to perform. Each element is one complete set of \
                                this tool's arguments.",
                "items": operation,
            }
        },
        "required": [FAN_OUT_KEY],
        "additionalProperties": false,
    });

    if let (Some(object), Some(definitions)) = (envelope.as_object_mut(), definitions) {
        object.extend(definitions);
    }

    envelope
}

/// What a call to a fan-out tool carries.
#[derive(Debug, Clone, PartialEq)]
pub enum Split {
    /// One operation's arguments, not an envelope: run as an ordinary call.
    Bare,

    /// An envelope, taken apart into one argument map per operation.
    Envelope(Vec<Map<String, Value>>),

    /// An envelope that cannot be taken apart.
    Malformed(ExpandError),
}

/// Decide whether a fan-out tool's call is an envelope or an ordinary call.
///
/// For a tool whose own schema does not declare a top-level [`FAN_OUT_KEY`]
/// parameter, the key alone decides: present means an envelope, which is
/// malformed if it is not a non-empty array of objects.
///
/// For a tool that does declare one, the arguments are an envelope only if the
/// key holds a non-empty array of objects that each validate against the tool's
/// own schema.
/// Anything else is the tool's own parameter, and the call is bare.
/// When both readings validate, the envelope wins: it is the shape every caller
/// was shown.
#[must_use]
pub fn split(definition: &ToolDefinition, arguments: &Map<String, Value>) -> Split {
    let declares_key = Node::root(&definition.parameters)
        .properties()
        .iter()
        .any(|(name, _)| name == FAN_OUT_KEY);

    match expand(arguments) {
        Err(ExpandError::Missing) => Split::Bare,
        Err(_) if declares_key => Split::Bare,
        Err(error) => Split::Malformed(error),
        Ok(operations) if declares_key && !all_validate(definition, &operations) => Split::Bare,
        Ok(operations) => Split::Envelope(operations),
    }
}

/// Whether every operation is a valid call to the tool on its own.
fn all_validate(definition: &ToolDefinition, operations: &[Map<String, Value>]) -> bool {
    operations.iter().all(|operation| {
        let mut operation = operation.clone();
        validate_arguments(definition, &mut operation).is_ok()
    })
}

/// Why an envelope could not be taken apart into operations.
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
    /// The message handed back to the caller.
    ///
    /// Each one names the envelope explicitly, because the caller reaching this
    /// point was shown the envelope schema and got the shape wrong.
    #[must_use]
    pub fn message(&self, tool_name: &str) -> String {
        match self {
            Self::Missing => format!(
                "Tool '{tool_name}' takes its operations in an `{FAN_OUT_KEY}` array, but the \
                 call had no `{FAN_OUT_KEY}` key."
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

/// Take an envelope apart into one argument map per operation.
///
/// This reads the shape only; [`split`] decides whether the arguments are an
/// envelope at all.
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

    /// The operation ran, or was resolved before running, and reported an
    /// error.
    Error(String),

    /// The operation never started, because an earlier one failed under
    /// `on_error = "stop"`.
    NotRun {
        /// One-based position of the operation whose failure stopped the rest.
        after: usize,
    },
}

/// Fold per-operation outcomes into the single result the caller receives.
///
/// Each operation gets a header naming its position, so a model reading the
/// result can line each section up with the operation it wrote.
/// Operations that never started say so explicitly: without that, a model that
/// asked for five and reads three assumes the other two succeeded silently.
///
/// A single operation is returned bare, with no framing at all, so a
/// one-operation call reads exactly like a call to the same tool without
/// fan-out, success or failure.
/// Otherwise the result is an error only when no operation succeeded.
#[must_use]
pub fn fold(outcomes: &[OperationOutcome]) -> ToolResult {
    match outcomes {
        [OperationOutcome::Ok(content)] => return ToolResult::text(content.clone()),
        [OperationOutcome::Error(message)] => return ToolResult::error(message.clone()),
        _ => {}
    }

    let body = frame(outcomes);
    if outcomes
        .iter()
        .any(|outcome| matches!(outcome, OperationOutcome::Ok(_)))
    {
        ToolResult::text(body)
    } else {
        ToolResult::error(body)
    }
}

/// Frame each operation's outcome under a header naming its position.
fn frame(outcomes: &[OperationOutcome]) -> String {
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

#[cfg(test)]
#[path = "fan_out_tests.rs"]
mod tests;
