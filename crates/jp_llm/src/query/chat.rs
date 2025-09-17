use jp_config::assistant::tool_choice::ToolChoice;
use jp_conversation::thread::Thread;

use crate::tool::ToolDefinition;

#[derive(Debug, Clone, Default)]
pub struct ChatQuery {
    pub thread: Thread,
    pub tools: Vec<ToolDefinition>,
    pub tool_choice: ToolChoice,
    pub tool_call_strict_mode: bool,
}
