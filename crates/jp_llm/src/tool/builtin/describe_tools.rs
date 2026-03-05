//! The `describe_tools` builtin implementation.

use async_trait::async_trait;
use indexmap::IndexMap;
use jp_tool::Outcome;
use serde_json::Value;

use crate::tool::{BuiltinTool, ToolDocs};

pub struct DescribeTools {
    docs: IndexMap<String, ToolDocs>,
}

impl DescribeTools {
    #[must_use]
    pub fn new(docs: IndexMap<String, ToolDocs>) -> Self {
        Self { docs }
    }

    fn format_tool_docs(name: &str, docs: &ToolDocs) -> String {
        let mut out = format!("## {name}\n");

        if let Some(summary) = &docs.summary {
            out.push('\n');
            out.push_str(summary);
            out.push('\n');
        }

        if let Some(desc) = &docs.description {
            out.push('\n');
            out.push_str(desc);
            out.push('\n');
        }

        if let Some(examples) = &docs.examples {
            out.push_str("\n### Examples\n\n");
            out.push_str(examples);
            out.push('\n');
        }

        let has_param_docs = docs.parameters.values().any(|p| !p.is_empty());
        if has_param_docs {
            out.push_str("\n### Parameters\n");

            for (param_name, param_docs) in &docs.parameters {
                if param_docs.is_empty() {
                    continue;
                }

                out.push_str(&format!("\n#### `{param_name}`\n"));

                if let Some(summary) = &param_docs.summary {
                    out.push('\n');
                    out.push_str(summary);
                    out.push('\n');
                }

                if let Some(desc) = &param_docs.description {
                    out.push('\n');
                    out.push_str(desc);
                    out.push('\n');
                }

                if let Some(examples) = &param_docs.examples {
                    out.push('\n');
                    out.push_str(examples);
                    out.push('\n');
                }
            }
        }

        out
    }
}

#[async_trait]
impl BuiltinTool for DescribeTools {
    async fn execute(&self, arguments: &Value, _answers: &IndexMap<String, Value>) -> Outcome {
        let tool_names = match arguments.get("tools").and_then(Value::as_array) {
            Some(arr) => arr.iter().filter_map(Value::as_str).collect::<Vec<_>>(),
            None => {
                return Outcome::Error {
                    message: "Missing or invalid `tools` parameter.".to_owned(),
                    trace: vec![],
                    transient: false,
                };
            }
        };

        if tool_names.is_empty() {
            return Outcome::Error {
                message: "The `tools` array must not be empty.".to_owned(),
                trace: vec![],
                transient: false,
            };
        }

        let mut sections = Vec::new();
        let mut not_found = Vec::new();

        for name in &tool_names {
            match self.docs.get(*name) {
                Some(docs) => sections.push(Self::format_tool_docs(name, docs)),
                None => not_found.push(*name),
            }
        }

        let mut output = sections.join("\n---\n\n");

        if !not_found.is_empty() {
            if !output.is_empty() {
                output.push_str("\n---\n\n");
            }
            output.push_str(&format!(
                "No additional documentation available for: {}",
                not_found.join(", ")
            ));
        }

        Outcome::Success { content: output }
    }
}

#[cfg(test)]
#[path = "describe_tools_tests.rs"]
mod tests;
