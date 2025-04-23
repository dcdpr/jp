use std::collections::HashMap;

use serde::{Deserialize, Serialize};

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
    pub description: Option<String>,
    /// See: <https://platform.openai.com/docs/guides/function-calling>
    pub parameters: HashMap<String, serde_json::Value>,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: Option<String>,
    pub arguments: Option<String>,
}
