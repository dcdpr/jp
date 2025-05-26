use std::{error::Error, sync::Arc};

use jp_conversation::thread::Thread;
use jp_mcp::{tool::ToolChoice, Tool};
use schemars::Schema;
use serde_json::{Map, Value};

type Mapping = Box<dyn Fn(&mut Value) -> Option<Value> + Send>;

/// A structured query for LLMs.
pub struct StructuredQuery {
    /// The thread to use for the query.
    pub thread: Thread,

    /// The JSON schema to enforce the shape of the response.
    schema: Map<String, Value>,

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
    pub fn new(schema: Schema, thread: Thread) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let Value::Object(schema) = schema.to_value() else {
            return Err("schema must be an object".into());
        };

        Ok(Self {
            thread,
            schema,
            mapping: None,
        })
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
    pub fn tool(&self) -> Tool {
        Tool {
            name: "generate_structured_data".into(),
            input_schema: Arc::new(self.schema.clone()),
            description: Some("Generate structured data".into()),
            annotations: None,
        }
    }

    #[must_use]
    pub fn tool_choice(&self) -> ToolChoice {
        ToolChoice::Function(self.tool().name.to_string())
    }
}
