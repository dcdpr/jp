//! Building a tool's parameter schema from configuration.
//!
//! A tool's parameters are one JSON Schema object.
//! For an MCP tool that is the server's `inputSchema` with the user's
//! configured overrides applied; for a local or built-in tool it is generated
//! from configuration alone.
//!
//! Reading and validating the result lives in [`jp_tool::schema`], which knows
//! nothing about configuration.

use indexmap::IndexMap;
use jp_config::conversation::tool::{OneOrManyTypes, ToolParameterConfig};
use jp_tool::{
    Error,
    schema::{Node, format_types, merge_description, required_names, validate_types},
};
use serde_json::{Map, Value, json};

/// Build the parameters schema for a tool whose shape is defined entirely in
/// configuration.
///
/// Local and built-in tools have no upstream schema, so every parameter must
/// declare a type.
///
/// # Errors
///
/// Returns [`Error::InvalidSchema`] when a parameter declares no type, or one
/// the schema cannot carry.
pub fn from_config(
    path: &str,
    parameters: &IndexMap<String, ToolParameterConfig>,
) -> Result<Value, Error> {
    let mut properties = Map::new();
    let mut required = vec![];

    for (name, parameter) in parameters {
        let node = node_from_config(&format!("{path}.{name}"), parameter)?;
        if parameter.required.unwrap_or(false) {
            required.push(Value::String(name.clone()));
        }
        properties.insert(name.clone(), node);
    }

    Ok(object_schema(properties, required))
}

/// Apply configured overrides to a schema declared by an MCP server.
///
/// The server's document is preserved, including any `$defs` block.
/// An override may narrow a parameter, but may not contradict the type the
/// server declared.
///
/// # Errors
///
/// Returns [`Error::InvalidSchema`] when an override contradicts the type the
/// server declared, or declares one the schema cannot carry.
pub fn with_overrides(
    path: &str,
    source: &Value,
    overrides: &IndexMap<String, ToolParameterConfig>,
) -> Result<Value, Error> {
    let mut schema = source.as_object().cloned().unwrap_or_default();
    let source_required = required_names(source);

    let mut properties = source
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut required = source_required
        .iter()
        .map(|name| Value::String((*name).to_owned()))
        .collect::<Vec<_>>();

    for (name, override_config) in overrides {
        let path = format!("{path}.{name}");
        let node = match properties.get(name) {
            Some(node) => node_with_override(&path, node, source, override_config)?,
            None => node_from_config(&path, override_config)?,
        };
        properties.insert(name.clone(), node);

        // A server's requirement cannot be relaxed, only added to: dropping it
        // would produce calls that omit an argument the server expects.
        let named = Value::String(name.clone());
        if override_config.required == Some(true) && !required.contains(&named) {
            required.push(named);
        }
    }

    schema.insert("properties".to_owned(), Value::Object(properties));
    schema.insert("required".to_owned(), Value::Array(required));
    schema.insert("type".to_owned(), Value::String("object".to_owned()));

    Ok(Value::Object(schema))
}

fn object_schema(properties: Map<String, Value>, required: Vec<Value>) -> Value {
    json!({
        "type": "object",
        "properties": Value::Object(properties),
        "required": Value::Array(required),
    })
}

/// Build one schema node from configuration alone.
fn node_from_config(path: &str, config: &ToolParameterConfig) -> Result<Value, Error> {
    let kind = config.kind.as_ref().ok_or_else(|| Error::InvalidSchema {
        path: format!("{path}.type"),
        message: "local and built-in tool parameters must declare a type".to_owned(),
    })?;

    let mut node = Map::new();
    node.insert("type".to_owned(), types_to_json(kind));
    apply_config_fields(path, &mut node, &Value::Null, config)?;

    Ok(Value::Object(node))
}

/// Overlay configuration onto a node the source already declared.
fn node_with_override(
    path: &str,
    source: &Value,
    root: &Value,
    config: &ToolParameterConfig,
) -> Result<Value, Error> {
    let mut node = source.as_object().cloned().unwrap_or_default();

    if let Some(kind) = &config.kind {
        // The source keeps its own declaration; an override may restate it but
        // not contradict it, since the source owns the contract. Resolving
        // against the document is what lets a referenced type be compared.
        let declared = Node::root(root).child(source).types();
        if !declared.is_empty() && !types_match(&declared, kind) {
            return Err(Error::InvalidSchema {
                path: format!("{path}.type"),
                message: format!(
                    "MCP declares {}, but the configuration declares {}",
                    format_types(&declared),
                    format_types(&type_names(kind))
                ),
            });
        }
        validate_types(path, &type_names(kind))?;
        if declared.is_empty() {
            node.insert("type".to_owned(), types_to_json(kind));
        }
    }

    apply_config_fields(path, &mut node, root, config)?;

    Ok(Value::Object(node))
}

/// Apply the override fields shared by both construction paths.
///
/// `root` is the document nested nodes resolve against; it is [`Value::Null`]
/// when the schema is built from configuration alone.
fn apply_config_fields(
    path: &str,
    node: &mut Map<String, Value>,
    root: &Value,
    config: &ToolParameterConfig,
) -> Result<(), Error> {
    if let Some(default) = &config.default {
        node.insert("default".to_owned(), default.clone());
    }
    if let Some(enumeration) = &config.enumeration {
        if enumeration.is_empty() {
            node.remove("enum");
        } else {
            node.insert("enum".to_owned(), Value::Array(enumeration.clone()));
        }
    }
    if let Some(description) = config.summary.as_ref().or(config.description.as_ref()) {
        let source = node.get("description").and_then(Value::as_str);
        if let Some(merged) = merge_description(Some(description.clone()), source) {
            node.insert("description".to_owned(), Value::String(merged));
        }
    }

    if let Some(items) = config.items.as_deref() {
        let path = format!("{path}.items");
        let merged = match node.get("items") {
            Some(source) => node_with_override(&path, source, root, items)?,
            None => node_from_config(&path, items)?,
        };
        node.insert("items".to_owned(), merged);
    }

    if !config.properties.is_empty() {
        let mut properties = node
            .get("properties")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let mut required = node
            .get("required")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        for (name, property) in &config.properties {
            let path = format!("{path}.properties.{name}");
            let merged = match properties.get(name) {
                Some(source) => node_with_override(&path, source, root, property)?,
                None => node_from_config(&path, property)?,
            };
            properties.insert(name.clone(), merged);

            let named = Value::String(name.clone());
            if property.required == Some(true) && !required.contains(&named) {
                required.push(named);
            }
        }

        node.insert("properties".to_owned(), Value::Object(properties));
        if !required.is_empty() {
            node.insert("required".to_owned(), Value::Array(required));
        }
    }

    Ok(())
}

fn type_names(types: &OneOrManyTypes) -> Vec<String> {
    match types {
        OneOrManyTypes::One(type_) => vec![type_.clone()],
        OneOrManyTypes::Many(types) => types.clone(),
    }
}

fn types_to_json(types: &OneOrManyTypes) -> Value {
    match types {
        OneOrManyTypes::One(type_) => Value::String(type_.clone()),
        OneOrManyTypes::Many(types) => {
            Value::Array(types.iter().cloned().map(Value::String).collect())
        }
    }
}

/// Whether two type declarations describe the same set of JSON types.
///
/// JSON Schema type arrays are unordered, and a single-element array means the
/// same thing as a bare string, so `["null", "string"]`, `["string", "null"]`
/// and `"string"` all compare equal.
fn types_match(left: &[String], right: &OneOrManyTypes) -> bool {
    let normalize = |mut types: Vec<String>| {
        types.sort_unstable();
        types.dedup();
        types
    };

    normalize(left.to_vec()) == normalize(type_names(right))
}

#[cfg(test)]
#[path = "json_schema_tests.rs"]
mod tests;
