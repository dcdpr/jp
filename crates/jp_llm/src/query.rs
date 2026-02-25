use jp_config::assistant::tool_choice::ToolChoice;
use jp_conversation::thread::Thread;

use crate::tool::ToolDefinition;

#[derive(Debug, Clone)]
pub struct ChatQuery {
    pub thread: Thread,
    // TODO: Should this be taken from `thread.events`, if not, document why?
    //
    // I think it should, because the tools that are available to the LLM are
    // always represented by the configuration in the conversation stream. If a
    // user adds a new tool to a config file, that tool is not automatically
    // available in existing conversations (it will be in new ones), but will
    // only become available when `--tool` or `--cfg` is used.
    pub tools: Vec<ToolDefinition>,
    // TODO: Should this instead be a delta config on `thread.events`?
    //
    // Same logic applies here, I think?
    pub tool_choice: ToolChoice,
}

impl From<Thread> for ChatQuery {
    fn from(thread: Thread) -> Self {
        Self {
            thread,
            tools: vec![],
            tool_choice: ToolChoice::default(),
        }
    }
}
