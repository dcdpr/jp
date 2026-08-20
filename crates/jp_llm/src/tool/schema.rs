//! Resolved tool parameter schemas and validation.

use std::slice;

use indexmap::IndexMap;
use jp_config::conversation::tool::OneOrManyTypes;
use serde_json::{Map, Value};

use crate::error::ToolError;

/// A resolved JSON Schema node for one tool parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolParameterSchema {
    /// JSON types accepted by the parameter.
    pub kind: OneOrManyTypes,

    /// Value inserted when the argument is omitted.
    pub default: Option<Value>,

    /// Whether callers must provide the parameter.
    pub required: bool,

    /// Short description sent in the provider tool schema.
    pub summary: Option<String>,

    /// Detailed parameter documentation.
    pub description: Option<String>,

    /// Parameter usage examples.
    pub examples: Option<String>,

    /// Complete values accepted by this schema node.
    pub enumeration: Vec<Value>,

    /// Schema applied to each array element.
    pub items: Option<Box<ToolParameterSchema>>,

    /// Schemas for object properties.
    pub properties: IndexMap<String, ToolParameterSchema>,
}

impl ToolParameterSchema {
    /// Convert this node to JSON Schema.
    #[must_use]
    pub fn to_json_schema(&self) -> Value {
        let mut map = Map::new();
        map.insert("type".to_owned(), match &self.kind {
            OneOrManyTypes::One(type_) => type_.clone().into(),
            OneOrManyTypes::Many(types) => types.clone().into(),
        });

        if let Some(description) = self.summary.as_deref().or(self.description.as_deref()) {
            map.insert("description".to_owned(), description.into());
        }
        if let Some(default) = self.default.clone() {
            map.insert("default".to_owned(), default);
        }
        if !self.enumeration.is_empty() {
            map.insert("enum".to_owned(), self.enumeration.as_slice().into());
        }
        if let Some(items) = self.items.as_deref() {
            map.insert("items".to_owned(), items.to_json_schema());
        }
        if !self.properties.is_empty() {
            let properties = self
                .properties
                .iter()
                .map(|(name, schema)| (name.clone(), schema.to_json_schema()))
                .collect();
            let required = self
                .properties
                .iter()
                .filter(|(_, schema)| schema.required)
                .map(|(name, _)| Value::String(name.clone()))
                .collect::<Vec<_>>();
            map.insert("properties".to_owned(), Value::Object(properties));
            if !required.is_empty() {
                map.insert("required".to_owned(), Value::Array(required));
            }
        }

        Value::Object(map)
    }
}

pub(super) fn parameter_accepts_value(value: &Value, types: &OneOrManyTypes) -> bool {
    match value {
        Value::Null => types.has_type("null"),
        Value::Bool(_) => types.has_type("boolean"),
        Value::Number(number) => {
            types.has_type("number")
                || (types.has_type("integer") && (number.is_i64() || number.is_u64()))
        }
        Value::String(_) => types.has_type("string"),
        Value::Array(_) => types.has_type("array"),
        Value::Object(_) => types.has_type("object"),
    }
}

pub(super) fn validate_parameter_schema(
    path: &str,
    config: &ToolParameterSchema,
) -> Result<(), ToolError> {
    validate_types(path, &config.kind)?;

    if config.kind.has_type("array") && config.items.is_none() {
        return Err(ToolError::InvalidSchema {
            path: format!("{path}.items"),
            message: "array schemas must declare an item schema".to_owned(),
        });
    }

    if let Some(items) = config.items.as_deref() {
        if !config.kind.has_type("array") {
            return Err(ToolError::InvalidSchema {
                path: format!("{path}.items"),
                message: format!(
                    "`items` requires an array type, but the schema requires {}",
                    format_types(&config.kind)
                ),
            });
        }
        validate_parameter_schema(&format!("{path}.items"), items)?;
    }

    if !config.properties.is_empty() {
        if !config.kind.has_type("object") {
            return Err(ToolError::InvalidSchema {
                path: format!("{path}.properties"),
                message: format!(
                    "`properties` requires an object type, but the schema requires {}",
                    format_types(&config.kind)
                ),
            });
        }
        for (name, property) in &config.properties {
            validate_parameter_schema(&format!("{path}.properties.{name}"), property)?;
        }
    }

    for (index, value) in config.enumeration.iter().enumerate() {
        if config.enumeration[..index].contains(value) {
            return Err(ToolError::InvalidSchema {
                path: format!("{path}.enum"),
                message: format!("enum values must be unique; duplicate value {value}"),
            });
        }

        if parameter_accepts_value(value, &config.kind) {
            validate_value_against_schema(
                &format!("{path}.enum[{index}]"),
                value,
                config,
                "enum value",
            )?;
            continue;
        }

        let hint = if config.kind.has_type("array") && !value.is_array() {
            format!("; use `{path}.items.enum` to constrain array elements")
        } else {
            String::new()
        };
        return Err(ToolError::InvalidSchema {
            path: format!("{path}.enum"),
            message: format!(
                "enum value {value} has type {}, but the schema requires {}{hint}",
                value_type(value),
                format_types(&config.kind),
            ),
        });
    }

    if let Some(default) = &config.default {
        validate_value_against_schema(
            &format!("{path}.default"),
            default,
            config,
            "default value",
        )?;
    }

    Ok(())
}

/// Whether two type declarations describe the same set of JSON types.
///
/// JSON Schema type arrays are unordered, and a single-element array means the
/// same thing as a bare string, so `["null", "string"]`, `["string", "null"]`
/// and `"string"` all compare equal.
pub(super) fn types_match(left: &OneOrManyTypes, right: &OneOrManyTypes) -> bool {
    fn normalize(types: &OneOrManyTypes) -> Vec<&str> {
        let mut types = match types {
            OneOrManyTypes::One(type_) => vec![type_.as_str()],
            OneOrManyTypes::Many(types) => types.iter().map(String::as_str).collect(),
        };
        types.sort_unstable();
        types.dedup();
        types
    }

    normalize(left) == normalize(right)
}

pub(super) fn validate_types(path: &str, types: &OneOrManyTypes) -> Result<(), ToolError> {
    let types = match types {
        OneOrManyTypes::One(type_) => slice::from_ref(type_),
        OneOrManyTypes::Many(types) if types.is_empty() => {
            return Err(ToolError::InvalidSchema {
                path: format!("{path}.type"),
                message: "type arrays must contain at least one type".to_owned(),
            });
        }
        OneOrManyTypes::Many(types) => types.as_slice(),
    };

    for (index, type_) in types.iter().enumerate() {
        if !matches!(
            type_.as_str(),
            "array" | "boolean" | "integer" | "null" | "number" | "object" | "string"
        ) {
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

/// Validate a schema-declared value against the schema node it appears in.
///
/// Applies the node's type and `enum`, then recurses into array elements and
/// object properties so nested constraints are enforced at every depth.
/// `subject` names what is being checked (`default value`, `enum value`) for
/// the error message.
fn validate_value_against_schema(
    path: &str,
    value: &Value,
    schema: &ToolParameterSchema,
    subject: &str,
) -> Result<(), ToolError> {
    if !parameter_accepts_value(value, &schema.kind) {
        return Err(ToolError::InvalidSchema {
            path: path.to_owned(),
            message: format!(
                "{subject} {value} has type {}, but the schema requires {}",
                value_type(value),
                format_types(&schema.kind)
            ),
        });
    }

    if !schema.enumeration.is_empty() && !schema.enumeration.contains(value) {
        return Err(ToolError::InvalidSchema {
            path: path.to_owned(),
            message: format!("{subject} {value} is not allowed by the enum"),
        });
    }

    if let (Value::Array(values), Some(items)) = (value, schema.items.as_deref()) {
        for (index, value) in values.iter().enumerate() {
            validate_value_against_schema(&format!("{path}[{index}]"), value, items, subject)?;
        }
    }

    if let Value::Object(values) = value {
        for (name, property) in &schema.properties {
            let Some(value) = values.get(name) else {
                if property.required {
                    return Err(ToolError::InvalidSchema {
                        path: format!("{path}.{name}"),
                        message: format!("{subject} is missing required property `{name}`"),
                    });
                }
                continue;
            };
            validate_value_against_schema(&format!("{path}.{name}"), value, property, subject)?;
        }
    }

    Ok(())
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

pub(super) fn format_types(types: &OneOrManyTypes) -> String {
    match types {
        OneOrManyTypes::One(type_) => type_.clone(),
        OneOrManyTypes::Many(types) => types.join(" or "),
    }
}
