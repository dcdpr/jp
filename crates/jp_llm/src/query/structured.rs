use jp_config::{
    assistant::tool_choice::ToolChoice,
    conversation::tool::{OneOrManyTypes, ToolParameterConfig},
};
use jp_conversation::thread::Thread;
use schemars::Schema;
use serde_json::Value;

use crate::{structured::SCHEMA_TOOL_NAME, tool::ToolDefinition};

type Mapping = Box<dyn Fn(&mut Value) -> Option<Value> + Send>;

/// A structured query for LLMs.
pub struct StructuredQuery {
    /// The thread to use for the query.
    pub thread: Thread,

    /// The JSON schema to enforce the shape of the response.
    schema: Schema,

    /// An optional mapping function to mutate the response object into a
    /// different shape.
    mapping: Option<Mapping>,
}

impl std::fmt::Debug for StructuredQuery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StructuredQuery")
            .field("thread", &self.thread)
            .field("schema", &self.schema)
            .field("mapping", &"<function>")
            .finish()
    }
}

impl StructuredQuery {
    /// Create a new structured query.
    #[must_use]
    pub fn new(schema: Schema, thread: Thread) -> Self {
        Self {
            thread,
            schema,
            mapping: None,
        }
    }

    #[must_use]
    pub fn with_mapping(
        mut self,
        mapping: impl Fn(&mut Value) -> Option<Value> + Send + Sync + 'static,
    ) -> Self {
        self.mapping = Some(Box::new(mapping));
        self
    }

    #[must_use]
    pub fn map(&self, mut value: Value) -> Value {
        self.mapping
            .as_ref()
            .and_then(|f| f(&mut value))
            .unwrap_or(value)
    }

    #[must_use]
    pub fn tool_definition(&self) -> ToolDefinition {
        let mut description = "Generate structured data".to_owned();
        if let Some(desc) = self.schema.get("description").and_then(|v| v.as_str()) {
            description.push_str(&format!(" using the following description:\n\n{desc}"));
        }

        let required = self
            .schema
            .get("required")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>();

        let parameters = self
            .schema
            .get("properties")
            .and_then(|v| v.as_object())
            .into_iter()
            .flatten()
            .map(|(k, v)| {
                let kind = v
                    .get("type")
                    .and_then(|v| match v.clone() {
                        Value::String(v) => Some(v.into()),
                        Value::Array(v) => Some(
                            v.into_iter()
                                .filter_map(|v| match v {
                                    Value::String(v) => Some(v),
                                    _ => None,
                                })
                                .collect::<Vec<_>>()
                                .into(),
                        ),
                        _ => None,
                    })
                    .unwrap_or_else(|| OneOrManyTypes::One("object".to_owned()));

                let parameter = ToolParameterConfig {
                    kind,
                    required: required.contains(&k.as_str()),
                    description: v
                        .get("description")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned),
                    default: v.get("default").cloned(),
                    enumeration: v
                        .get("enum")
                        .and_then(|v| v.as_array().cloned())
                        .unwrap_or_default(),
                    items: v
                        .get("items")
                        .and_then(|v| serde_json::from_value(v.clone()).ok()),
                };

                (k.to_owned(), parameter)
            })
            .collect();

        ToolDefinition {
            name: SCHEMA_TOOL_NAME.to_owned(),
            description: Some(description),
            parameters,
        }
    }

    #[must_use]
    pub fn tool_choice(&self) -> ToolChoice {
        ToolChoice::Function(self.tool_definition().name)
    }
}
