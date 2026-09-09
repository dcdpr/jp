use jp_config::assistant::tool_choice::ToolChoice;
use jp_conversation::thread::Thread;

use crate::tool::ToolDefinition;

/// Whether the provider may drop input to make a request fit the model's
/// context window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Truncation {
    /// The provider may silently discard input and answer anyway.
    ///
    /// Suits a request whose answer stands on its own, where a degraded answer
    /// beats no answer.
    #[default]
    Allowed,

    /// The provider must reject a request that does not fit.
    ///
    /// Required when the answer is stored as standing for the input it was
    /// built from: a silently shortened request yields an answer that claims
    /// coverage it never had.
    Forbidden,
}

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

    /// Whether the provider may drop input to fit its context window.
    ///
    /// Only providers that offer the choice read this; the rest either always
    /// reject an oversized request or truncate server-side beyond JP's control.
    pub truncation: Truncation,
}

impl From<Thread> for ChatQuery {
    fn from(thread: Thread) -> Self {
        Self {
            thread,
            tools: vec![],
            tool_choice: ToolChoice::default(),
            truncation: Truncation::default(),
        }
    }
}
