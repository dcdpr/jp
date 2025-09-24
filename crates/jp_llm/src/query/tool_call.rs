use jp_config::assistant::tool_choice::ToolChoice;

use crate::tool::ToolDefinition;

#[derive(Debug, Clone)]
pub struct ToolCallResultQuery {
    pub tools: Vec<ToolDefinition>,
    pub tool_choice: ToolChoice,
}
