use jp_conversation::thread::Thread;
use jp_mcp::{tool::ToolChoice, Tool};

#[derive(Debug, Clone, Default)]
pub struct ChatQuery {
    pub thread: Thread,
    pub tools: Vec<Tool>,
    pub tool_choice: ToolChoice,
    pub tool_call_strict_mode: bool,
}
