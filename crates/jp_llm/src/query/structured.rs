use std::fmt;

use jp_config::{
    assistant::tool_choice::ToolChoice,
    conversation::tool::{OneOrManyTypes, ToolParameterConfig},
};
use jp_conversation::thread::Thread;
use schemars::Schema;
use serde_json::Value;

use crate::{Error, structured::SCHEMA_TOOL_NAME, tool::ToolDefinition};

type Mapping = Box<dyn Fn(&mut Value) -> Option<Value> + Send>;
type Validate = Box<dyn Fn(&Value) -> Result<(), String> + Send>;

/// A structured query for LLMs.
pub struct StructuredQuery {
    /// The thread to use for the query.
    pub thread: Thread,

    /// The JSON schema to enforce the shape of the response.
    schema: Schema,

    /// An optional mapping function to mutate the response object into a
    /// different shape.
    mapping: Option<Mapping>,

    /// Validators to run on the response. If a validator fails, its error is
    /// sent back to the assistant, so that it can be fixed/retried.
    ///
    /// TODO: Add support for JSON Schema validation.
    validators: Vec<Validate>,
}

impl fmt::Debug for StructuredQuery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StructuredQuery")
            .field("thread", &self.thread)
            .field("schema", &self.schema)
            .field("mapping", &"<function>")
            .field("validators", &"<functions>")
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
            validators: vec![],
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
    pub fn with_validator(
        mut self,
        validator: impl Fn(&Value) -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        self.validators.push(Box::new(validator));
        self
    }

    #[must_use]
    pub fn with_schema_validator(self, schema: Schema) -> Self {
        let validate = move |value: &Value| {
            jsonschema::validate(schema.as_value(), value).map_err(|e| e.to_string())
        };

        self.with_validator(validate)
    }

    #[must_use]
    pub fn map(&self, mut value: Value) -> Value {
        self.mapping
            .as_ref()
            .and_then(|f| f(&mut value))
            .unwrap_or(value)
    }

    pub fn validate(&self, value: &Value) -> Result<(), String> {
        for validator in &self.validators {
            validator(value)?;
        }

        Ok(())
    }

    pub fn tool_definition(&self) -> Result<ToolDefinition, Error> {
        let mut description =
            "This tool can be used to deliver structured data to the caller. It is NOT intended \
             to GENERATE the requested data, but instead as a structured delivery mechanism. The \
             tool is a no-op implementation in that it allows the assistant to deliver structured \
             data to the user, but the tool will never report back with a result, instead the \
             user can take the structured data from the tool arguments provided."
                .to_owned();
        if let Some(desc) = self.schema.get("description").and_then(|v| v.as_str()) {
            description.push_str(&format!(
                " Here is the description for the requested structured data:\n\n{desc}"
            ));
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
                        .map(|v| serde_json::from_value(v.clone()))
                        .transpose()?,
                };

                Ok((k.to_owned(), parameter))
            })
            .collect::<Result<_, Error>>()?;

        Ok(ToolDefinition {
            name: SCHEMA_TOOL_NAME.to_owned(),
            description: Some(description),
            parameters,
            include_tool_answers_parameter: false,
        })
    }

    pub fn tool_choice(&self) -> Result<ToolChoice, Error> {
        Ok(ToolChoice::Function(self.tool_definition()?.name))
    }
}
