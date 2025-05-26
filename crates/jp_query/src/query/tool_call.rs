use jp_mcp::{tool::ToolChoice, Tool};

#[derive(Debug, Clone)]
pub struct ToolCallResultQuery {
    pub tools: Vec<Tool>,
    pub tool_choice: ToolChoice,
}
