use jp_config::llm::ToolChoice;
use jp_mcp::Tool;

#[derive(Debug, Clone)]
pub struct ToolCallResultQuery {
    pub tools: Vec<Tool>,
    pub tool_choice: ToolChoice,
}
