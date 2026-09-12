//! What a tool is called, what it accepts, and how it is described.
//!
//! A [`ToolDefinition`] is the resolved description of one tool, whatever its
//! source: a local command, a built-in implementation, or a tool a configured
//! MCP server declares.
//! Building one reads configuration, so that belongs with the configuration
//! types; this module holds the resolved shape and the argument handling that
//! reads its schema.

use indexmap::IndexMap;
use serde_json::{Map, Value};

use crate::{Error, schema::Node};

/// Documentation for a single tool parameter.
#[derive(Debug, Clone)]
pub struct ParameterDocs {
    pub summary: Option<String>,
    pub description: Option<String>,
    pub examples: Option<String>,
}

impl ParameterDocs {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.description.is_none() && self.examples.is_none()
    }
}

/// Documentation for a single tool.
#[derive(Debug, Clone, Default)]
pub struct ToolDocs {
    pub summary: Option<String>,
    pub description: Option<String>,
    pub examples: Option<String>,
    pub parameters: IndexMap<String, ParameterDocs>,
}

impl ToolDocs {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.description.is_none()
            && self.examples.is_none()
            && self.parameters.values().all(ParameterDocs::is_empty)
    }

    /// The short description used for the tool schema sent to the LLM.
    ///
    /// Returns `summary` if set, otherwise falls back to `description`.
    #[must_use]
    pub fn schema_description(&self) -> Option<&str> {
        self.summary.as_deref().or(self.description.as_deref())
    }
}

/// The definition of a tool.
#[derive(Debug, Clone)]
pub struct ToolDefinition {
    pub name: String,
    pub docs: ToolDocs,

    /// JSON Schema for the tool's arguments, as its source declared it, with
    /// configuration overrides applied.
    ///
    /// Adapting this to what a given API accepts belongs to that provider.
    pub parameters: Value,
}

impl ToolDefinition {
    /// Coerce JSON-encoded argument strings to non-string schema types.
    ///
    /// Strings stay unchanged when the schema accepts strings or their contents
    /// do not parse to a declared type.
    pub fn coerce_arguments(&self, arguments: &mut Map<String, Value>) {
        coerce_arguments_to_schema(arguments, &self.parameters);
    }

    /// Return the JSON Schema for the tool's parameters.
    #[must_use]
    pub fn to_parameters_schema(&self) -> Value {
        self.parameters.clone()
    }
}

/// Split a description string into a short summary and remaining detail.
///
/// If the text is short (single line, ≤120 chars), it is returned as the
/// summary with no remaining description.
///
/// Otherwise, the first sentence is extracted as the summary.
/// A sentence ends at ` .  ` or `.\n`.
/// The remainder becomes the description.
#[must_use]
pub fn split_description(text: &str) -> (String, Option<String>) {
    let text = text.trim();

    // Find the first sentence boundary.
    // Look for ". " or ".\n" — a period followed by whitespace.
    for (i, _) in text.match_indices('.') {
        let after = i + 1;
        if after >= text.len() {
            // Period at end of string — the whole text is one sentence.
            break;
        }

        let next_byte = text.as_bytes()[after];
        if next_byte == b'\n' {
            // Period followed by newline is always a sentence boundary.
        } else if next_byte == b' ' {
            // Period followed by space: only split if the next non-space
            // character is uppercase (heuristic to skip abbreviations
            // like "e.g. foo").
            let rest_after_space = text[after..].trim_start();
            if rest_after_space.is_empty()
                || !rest_after_space
                    .chars()
                    .next()
                    .is_some_and(char::is_uppercase)
            {
                continue;
            }
        } else {
            continue;
        }

        {
            let summary = text[..=i].trim().to_owned();
            let rest = text[after..].trim();

            if rest.is_empty() {
                return (summary, None);
            }

            return (summary, Some(rest.to_owned()));
        }
    }

    // No sentence boundary found — take the first line.
    if let Some(nl) = text.find('\n') {
        let summary = text[..nl].trim().to_owned();
        let rest = text[nl..].trim();

        if rest.is_empty() {
            return (summary, None);
        }

        return (summary, Some(rest.to_owned()));
    }

    // Single long line, no period — return as-is.
    (text.to_owned(), None)
}

/// Coerce JSON-encoded argument strings to the types the schema declares.
fn coerce_arguments_to_schema(arguments: &mut Map<String, Value>, schema: &Value) {
    coerce_object(arguments, &Node::root(schema));
}

fn coerce_object(arguments: &mut Map<String, Value>, node: &Node<'_>) {
    for (name, property) in node.properties() {
        if let Some(value) = arguments.get_mut(&name) {
            coerce_value(value, &property);
        }
    }
}

fn coerce_value(value: &mut Value, node: &Node<'_>) {
    // Coercion repairs an argument the schema cannot take as written. A
    // parameter that permits the string has nothing to repair, so parsing it
    // would hand the tool a number or an object where the model sent text.
    if let Value::String(raw) = &*value
        && !node.permits(value)
        && let Ok(parsed) = serde_json::from_str::<Value>(raw)
        && node.permits(&parsed)
    {
        *value = parsed;
    }

    match value {
        Value::Object(arguments) => coerce_object(arguments, node),
        Value::Array(values) => {
            let Some(items) = node.items() else {
                return;
            };
            for value in values {
                coerce_value(value, &items);
            }
        }
        _ => {}
    }
}

/// Fill in configured default values for missing parameters.
///
/// LLMs commonly omit parameters that have a `default` in the JSON schema, even
/// when those parameters are marked `required`.
/// This function patches the arguments map before validation so that such
/// omissions don't cause spurious "missing argument" errors and unnecessary LLM
/// retries.
pub fn apply_parameter_defaults(arguments: &mut Map<String, Value>, schema: &Value) {
    apply_defaults_to(arguments, &Node::root(schema));
}

fn apply_defaults_to(arguments: &mut Map<String, Value>, node: &Node<'_>) {
    for (name, property) in node.properties() {
        if !arguments.contains_key(&name) {
            if let Some(default) = property.default() {
                let default = default.clone();
                arguments.insert(name, default);
            }
            continue;
        }

        // Recurse into object fields.
        if property.has_properties()
            && let Some(object) = arguments.get_mut(&name).and_then(Value::as_object_mut)
        {
            apply_defaults_to(object, &property);
        }

        // Recurse into array elements.
        if let Some(items) = property.items()
            && items.has_properties()
            && let Some(values) = arguments.get_mut(&name).and_then(Value::as_array_mut)
        {
            for value in values.iter_mut() {
                if let Some(object) = value.as_object_mut() {
                    apply_defaults_to(object, &items);
                }
            }
        }
    }
}

/// Check a call's arguments against the tool's parameters schema.
///
/// # Errors
///
/// Returns [`Error::Arguments`] naming every required argument that is absent
/// and every argument the schema does not declare.
pub fn validate_tool_arguments(
    arguments: &Map<String, Value>,
    schema: &Value,
) -> Result<(), Error> {
    validate_arguments_against(arguments, &Node::root(schema))
}

fn validate_arguments_against(
    arguments: &Map<String, Value>,
    node: &Node<'_>,
) -> Result<(), Error> {
    let properties = node.properties();

    let unknown = arguments
        .keys()
        .filter(|name| !properties.iter().any(|(known, _)| known == *name))
        .cloned()
        .collect::<Vec<_>>();

    let missing = properties
        .iter()
        .filter(|(name, _)| node.is_required(name) && !arguments.contains_key(name))
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();

    if !missing.is_empty() || !unknown.is_empty() {
        return Err(Error::Arguments { missing, unknown });
    }

    // Recurse into nested structures.
    for (name, property) in properties {
        let Some(value) = arguments.get(&name) else {
            continue;
        };

        if let Some(object) = value.as_object()
            && property.has_properties()
        {
            validate_arguments_against(object, &property)?;
        }

        if let Some(items) = property.items()
            && items.has_properties()
            && let Some(values) = value.as_array()
        {
            for value in values {
                if let Some(object) = value.as_object() {
                    validate_arguments_against(object, &items)?;
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
#[path = "definition_tests.rs"]
mod tests;
