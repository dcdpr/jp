//! JSON Schema for tool parameters: construction, validation, and reading.
//!
//! A tool's parameters are one JSON Schema object, held exactly as its source
//! declared it.
//! For an MCP tool that is the server's `inputSchema` with the user's
//! configured overrides applied; for a local or built-in tool it is generated
//! from configuration.
//! Nothing else rewrites it: adapting a schema to what a given API accepts is
//! the responsibility of that provider.
//!
//! [`Node`] is the read-only view used by argument handling and validation.
//! It follows same-document `$ref` pointers while reading, so a referenced enum
//! or nested object answers questions the same way an inline one does.

use std::borrow::Cow;

use indexmap::IndexMap;
use jp_config::conversation::tool::{OneOrManyTypes, ToolParameterConfig};
use serde_json::{Map, Value, json};

use crate::error::ToolError;

/// JSON types a tool parameter may declare.
const SUPPORTED_TYPES: &[&str] = &[
    "array", "boolean", "integer", "null", "number", "object", "string",
];

/// Bound on `$ref` expansion while reading, so a self-referential schema
/// terminates.
const MAX_REF_HOPS: usize = 32;

/// Build the parameters schema for a tool whose shape is defined entirely in
/// configuration.
///
/// Local and built-in tools have no upstream schema, so every parameter must
/// declare a type.
pub fn from_config(
    path: &str,
    parameters: &IndexMap<String, ToolParameterConfig>,
) -> Result<Value, ToolError> {
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
pub fn with_overrides(
    path: &str,
    source: &Value,
    overrides: &IndexMap<String, ToolParameterConfig>,
) -> Result<Value, ToolError> {
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

/// Validate a tool's parameters schema.
///
/// Rejects the shapes that no provider can act on, and the ones that contradict
/// themselves: unusable types, arrays with no item schema, `items` or
/// `properties` on a type that cannot carry them, duplicate or ill-typed enum
/// values, and defaults the schema itself forbids.
pub fn validate(path: &str, schema: &Value) -> Result<(), ToolError> {
    let root = Node::root(schema);
    for (name, property) in root.properties() {
        validate_node(&format!("{path}.{name}"), &property, &mut vec![])?;
    }

    Ok(())
}

/// Validate one node, tracking which definitions the walk is already inside.
///
/// A recursive schema is legal, and providers that reject it say so themselves.
/// Re-entering a definition already on the path adds nothing, so the walk stops
/// there instead of expanding forever.
fn validate_node(path: &str, node: &Node<'_>, visiting: &mut Vec<String>) -> Result<(), ToolError> {
    if let Some(origin) = node.origin() {
        if visiting.iter().any(|seen| seen == origin) {
            return Ok(());
        }
        visiting.push(origin.to_owned());
    }

    let result = validate_node_inner(path, node, visiting);

    if node.origin().is_some() {
        visiting.pop();
    }

    result
}

fn validate_node_inner(
    path: &str,
    node: &Node<'_>,
    visiting: &mut Vec<String>,
) -> Result<(), ToolError> {
    let types = node.types();
    validate_types(path, &types)?;

    let items = node.items();
    if types.iter().any(|type_| type_ == "array") && items.is_none() {
        return Err(ToolError::InvalidSchema {
            path: format!("{path}.items"),
            message: "array schemas must declare an item schema".to_owned(),
        });
    }

    if let Some(items) = &items {
        if !types.iter().any(|type_| type_ == "array") {
            return Err(ToolError::InvalidSchema {
                path: format!("{path}.items"),
                message: format!(
                    "`items` requires an array type, but the schema requires {}",
                    format_types(&types)
                ),
            });
        }
        validate_node(&format!("{path}.items"), items, visiting)?;
    }

    let properties = node.properties();
    if !properties.is_empty() && !types.iter().any(|type_| type_ == "object") {
        return Err(ToolError::InvalidSchema {
            path: format!("{path}.properties"),
            message: format!(
                "`properties` requires an object type, but the schema requires {}",
                format_types(&types)
            ),
        });
    }
    for (name, property) in properties {
        validate_node(&format!("{path}.properties.{name}"), &property, visiting)?;
    }

    let enumeration = node.enumeration();
    for (index, value) in enumeration.iter().enumerate() {
        if enumeration[..index].contains(value) {
            return Err(ToolError::InvalidSchema {
                path: format!("{path}.enum"),
                message: format!("enum values must be unique; duplicate value {value}"),
            });
        }

        if node.accepts(value) {
            validate_value(&format!("{path}.enum[{index}]"), value, node, "enum value")?;
            continue;
        }

        let hint = if types.iter().any(|type_| type_ == "array") && !value.is_array() {
            format!("; use `{path}.items.enum` to constrain array elements")
        } else {
            String::new()
        };
        return Err(ToolError::InvalidSchema {
            path: format!("{path}.enum"),
            message: format!(
                "enum value {value} has type {}, but the schema requires {}{hint}",
                value_type(value),
                format_types(&types),
            ),
        });
    }

    if let Some(default) = node.default() {
        validate_value(&format!("{path}.default"), default, node, "default value")?;
    }

    Ok(())
}

/// Validate a schema-declared value against the node it appears in.
///
/// Applies the node's type and `enum`, then recurses into array elements and
/// object properties so nested constraints are enforced at every depth.
/// `subject` names what is being checked (`default value`, `enum value`) for
/// the error message.
fn validate_value(
    path: &str,
    value: &Value,
    node: &Node<'_>,
    subject: &str,
) -> Result<(), ToolError> {
    if !node.accepts(value) {
        return Err(ToolError::InvalidSchema {
            path: path.to_owned(),
            message: format!(
                "{subject} {value} has type {}, but the schema requires {}",
                value_type(value),
                format_types(&node.types())
            ),
        });
    }

    let enumeration = node.enumeration();
    if !enumeration.is_empty() && !enumeration.contains(value) {
        return Err(ToolError::InvalidSchema {
            path: path.to_owned(),
            message: format!("{subject} {value} is not allowed by the enum"),
        });
    }

    if let (Value::Array(values), Some(items)) = (value, node.items()) {
        for (index, value) in values.iter().enumerate() {
            validate_value(&format!("{path}[{index}]"), value, &items, subject)?;
        }
    }

    if let Value::Object(values) = value {
        for (name, property) in node.properties() {
            let Some(value) = values.get(&name) else {
                if node.is_required(&name) {
                    return Err(ToolError::InvalidSchema {
                        path: format!("{path}.{name}"),
                        message: format!("{subject} is missing required property `{name}`"),
                    });
                }
                continue;
            };
            validate_value(&format!("{path}.{name}"), value, &property, subject)?;
        }
    }

    Ok(())
}

fn validate_types(path: &str, types: &[String]) -> Result<(), ToolError> {
    if types.is_empty() {
        return Err(ToolError::InvalidSchema {
            path: format!("{path}.type"),
            message: "schema does not declare a supported type".to_owned(),
        });
    }

    for (index, type_) in types.iter().enumerate() {
        if !SUPPORTED_TYPES.contains(&type_.as_str()) {
            return Err(ToolError::InvalidSchema {
                path: format!("{path}.type"),
                message: format!("unsupported JSON type `{type_}`"),
            });
        }
        if types[..index].contains(type_) {
            return Err(ToolError::InvalidSchema {
                path: format!("{path}.type"),
                message: format!("type values must be unique; duplicate type `{type_}`"),
            });
        }
    }

    Ok(())
}

/// Expand every same-document `$ref` and drop the definitions block.
///
/// For providers that cannot follow references.
/// A reference that cannot be resolved, or one that revisits a definition
/// already being expanded, is left in place: a recursive type has no finite
/// expansion, and dropping the node would be worse than forwarding something
/// the API can reject.
#[must_use]
pub fn inline(schema: &Value) -> Value {
    let mut inlined = inline_node(schema, schema, &mut vec![]);
    if let Some(object) = inlined.as_object_mut() {
        object.remove("$defs");
        object.remove("definitions");
    }

    inlined
}

fn inline_node(node: &Value, root: &Value, expanding: &mut Vec<String>) -> Value {
    let pointer = pointer_of(node);
    if let Some(pointer) = &pointer {
        if expanding.contains(pointer) {
            return node.clone();
        }
        expanding.push(pointer.clone());
    }

    let resolved = resolve(node, root);
    let expanded = match resolved.as_object() {
        Some(object) => Value::Object(
            object
                .iter()
                .map(|(key, value)| {
                    let value = match value {
                        Value::Object(_) => inline_node(value, root, expanding),
                        Value::Array(values) => Value::Array(
                            values
                                .iter()
                                .map(|value| inline_node(value, root, expanding))
                                .collect(),
                        ),
                        other => other.clone(),
                    };
                    (key.clone(), value)
                })
                .collect(),
        ),
        None => resolved.into_owned(),
    };

    if pointer.is_some() {
        expanding.pop();
    }

    expanded
}

/// A read-only view of one schema node, resolving `$ref` as it reads.
#[derive(Debug, Clone)]
pub struct Node<'a> {
    root: &'a Value,
    node: Cow<'a, Value>,
    origin: Option<String>,
}

impl<'a> Node<'a> {
    /// View a whole parameters schema, where `$ref` pointers resolve against
    /// the same document.
    #[must_use]
    pub fn root(schema: &'a Value) -> Self {
        Self {
            root: schema,
            node: resolve(schema, schema),
            origin: pointer_of(schema),
        }
    }

    /// The `$ref` pointer this node was reached through, when it was one.
    #[must_use]
    pub fn origin(&self) -> Option<&str> {
        self.origin.as_deref()
    }

    /// View a nested node, resolving it against the same document.
    ///
    /// The node is cloned because resolving a `$ref` produces a new value that
    /// cannot borrow from the parent.
    fn child(&self, node: &Value) -> Node<'a> {
        Node {
            root: self.root,
            node: Cow::Owned(resolve(node, self.root).into_owned()),
            origin: pointer_of(node),
        }
    }

    /// JSON types this node accepts.
    #[must_use]
    pub fn types(&self) -> Vec<String> {
        match self.node.get("type") {
            Some(Value::String(type_)) => vec![type_.clone()],
            Some(Value::Array(types)) => types
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect(),
            _ => vec![],
        }
    }

    /// Whether a value satisfies this node's declared types.
    #[must_use]
    pub fn accepts(&self, value: &Value) -> bool {
        let types = self.types();
        let has = |type_: &str| types.iter().any(|candidate| candidate == type_);

        match value {
            Value::Null => has("null"),
            Value::Bool(_) => has("boolean"),
            Value::Number(number) => {
                has("number") || (has("integer") && (number.is_i64() || number.is_u64()))
            }
            Value::String(_) => has("string"),
            Value::Array(_) => has("array"),
            Value::Object(_) => has("object"),
        }
    }

    /// The value inserted when the argument is omitted.
    #[must_use]
    pub fn default(&self) -> Option<&Value> {
        // The borrow has to come from the node itself, which `Cow` owns when a
        // `$ref` was inlined, so match rather than returning through the Cow.
        match &self.node {
            Cow::Borrowed(node) => node.get("default"),
            Cow::Owned(node) => node.get("default"),
        }
    }

    /// Values this node accepts, empty when unconstrained.
    #[must_use]
    pub fn enumeration(&self) -> Vec<Value> {
        self.node
            .get("enum")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// The schema applied to each array element.
    #[must_use]
    pub fn items(&self) -> Option<Node<'a>> {
        self.node.get("items").map(|items| self.child(items))
    }

    /// The schemas for this node's object properties, in declaration order.
    #[must_use]
    pub fn properties(&self) -> Vec<(String, Node<'a>)> {
        self.node
            .get("properties")
            .and_then(Value::as_object)
            .map(|properties| {
                properties
                    .iter()
                    .map(|(name, node)| (name.clone(), self.child(node)))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether this node lists `name` among its required properties.
    #[must_use]
    pub fn is_required(&self, name: &str) -> bool {
        required_names(&self.node).contains(&name)
    }

    /// The description sent to the model for this node.
    #[must_use]
    pub fn description(&self) -> Option<&str> {
        match &self.node {
            Cow::Borrowed(node) => node.get("description"),
            Cow::Owned(node) => node.get("description"),
        }
        .and_then(Value::as_str)
    }

    /// Whether this node declares any property.
    #[must_use]
    pub fn has_properties(&self) -> bool {
        self.node
            .get("properties")
            .and_then(Value::as_object)
            .is_some_and(|properties| !properties.is_empty())
    }
}

impl PartialEq for Node<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.node == other.node
    }
}

/// Follow same-document `$ref` pointers, merging sibling keys over the target.
///
/// Sibling keys win, per JSON Schema 2020-12.
/// A pointer that leaves the document or revisits one already followed is left
/// in place, so reading degrades to "this node declares nothing" rather than
/// looping.
fn resolve<'a>(node: &'a Value, root: &Value) -> Cow<'a, Value> {
    let mut current = Cow::Borrowed(node);
    let mut seen: Vec<String> = vec![];

    while let Some(pointer) = current.get("$ref").and_then(Value::as_str) {
        let pointer = pointer.to_owned();
        if seen.len() >= MAX_REF_HOPS || seen.contains(&pointer) {
            break;
        }

        let Some(target) = follow_pointer(&pointer, root) else {
            break;
        };

        let mut merged = target;
        for (key, value) in current.as_object().into_iter().flatten() {
            if key != "$ref" {
                merged.insert(key.clone(), value.clone());
            }
        }

        seen.push(pointer);
        current = Cow::Owned(Value::Object(merged));
    }

    current
}

fn pointer_of(node: &Value) -> Option<String> {
    node.get("$ref").and_then(Value::as_str).map(str::to_owned)
}

/// Look up a same-document JSON pointer, such as `#/$defs/EntryType`.
fn follow_pointer(pointer: &str, root: &Value) -> Option<Map<String, Value>> {
    if pointer == "#" {
        return root.as_object().cloned();
    }

    let mut current = root;
    for segment in pointer.strip_prefix("#/")?.split('/') {
        current = current.get(segment.replace("~1", "/").replace("~0", "~"))?;
    }

    current.as_object().cloned()
}

fn required_names(schema: &Value) -> Vec<&str> {
    schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

fn object_schema(properties: Map<String, Value>, required: Vec<Value>) -> Value {
    json!({
        "type": "object",
        "properties": Value::Object(properties),
        "required": Value::Array(required),
    })
}

/// Build one schema node from configuration alone.
fn node_from_config(path: &str, config: &ToolParameterConfig) -> Result<Value, ToolError> {
    let kind = config
        .kind
        .as_ref()
        .ok_or_else(|| ToolError::InvalidSchema {
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
) -> Result<Value, ToolError> {
    let mut node = source.as_object().cloned().unwrap_or_default();

    if let Some(kind) = &config.kind {
        // The source keeps its own declaration; an override may restate it but
        // not contradict it, since the source owns the contract. Resolving
        // against the document is what lets a referenced type be compared.
        let declared = Node::root(root).child(source).types();
        if !declared.is_empty() && !types_match(&declared, kind) {
            return Err(ToolError::InvalidSchema {
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
) -> Result<(), ToolError> {
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

/// Merge a user-provided description with the one the source declared.
///
/// A user description containing `{{description}}` has the source's text
/// substituted in; otherwise the user's text wins outright.
/// With no user description the source's is kept as-is.
#[must_use]
pub fn merge_description(user: Option<String>, source: Option<&str>) -> Option<String> {
    match (user, source) {
        (None, Some(source)) => Some(source.to_owned()),
        // TODO: should use `minijinja` instead of raw string replacement.
        (Some(user), Some(source)) => Some(user.replace("{{description}}", source)),
        (Some(user), None) => Some(user),
        (None, None) => None,
    }
}

fn value_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(number) if number.is_i64() || number.is_u64() => "integer",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn format_types(types: &[String]) -> String {
    types.join(" or ")
}

#[cfg(test)]
#[path = "json_schema_tests.rs"]
mod tests;
