use serde::{Deserialize, Serialize};
use serde_json::Map;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub content: String,
    pub tool_call_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename = "lowercase", rename_all = "snake_case", tag = "type")]
pub enum Tool {
    Function { function: ToolFunction },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename = "lowercase", rename_all = "snake_case")]
pub struct ToolFunction {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "strict_is_false")]
    pub strict: bool,
    /// See: <https://platform.openai.com/docs/guides/function-calling>
    pub parameters: Map<String, serde_json::Value>,
}

#[expect(clippy::trivially_copy_pass_by_ref)]
fn strict_is_false(strict: &bool) -> bool {
    !strict
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum ToolCall {
    Function {
        id: Option<String>,
        index: usize,
        function: FunctionCall,
    },
}

impl ToolCall {
    #[must_use]
    pub fn id(&self) -> Option<String> {
        match self {
            Self::Function { id, .. } => id.clone(),
        }
    }

    #[must_use]
    pub fn name(&self) -> Option<String> {
        match self {
            Self::Function { function, .. } => function.name.clone(),
        }
    }

    #[must_use]
    pub fn arguments(&self) -> Option<String> {
        match self {
            Self::Function { function, .. } => function.arguments.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: Option<String>,
    pub arguments: Option<String>,
}

mod strings {
    crate::named_unit_variant!(auto);
    crate::named_unit_variant!(none);
    crate::named_unit_variant!(required);
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolChoice {
    /// Call zero, one, or multiple tools, at the discretion of the LLM.
    #[default]
    #[serde(with = "strings::auto")]
    Auto,

    /// Force the LLM not to call any tools, even if any are available.
    #[serde(with = "strings::none")]
    None,

    /// Force the LLM to call at least one tool.
    #[serde(with = "strings::required")]
    Required,

    /// Require calling the specified named tool.
    Function(ToolChoiceFunction),
}

impl ToolChoice {
    pub fn function(name: impl Into<String>) -> Self {
        Self::Function(ToolChoiceFunction {
            function: ChoiceFunction { name: name.into() },
            ..Default::default()
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ToolChoiceFunction {
    #[serde(rename = "type")]
    pub kind: ChoiceFunctionType,
    pub function: ChoiceFunction,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChoiceFunctionType {
    #[default]
    Function,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChoiceFunction {
    pub name: String,
}
