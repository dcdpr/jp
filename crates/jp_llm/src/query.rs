use camino::Utf8PathBuf;
use jp_config::assistant::tool_choice::ToolChoice;
use jp_conversation::thread::Thread;
use jp_mcp::server::InvocationContext;
use jp_tool::ToolDefinition;
use url::Url;

use crate::stream::EventStream;

/// Host resources available to a provider-owned continuation loop.
#[derive(Debug, Clone)]
pub struct QueryContext {
    /// Stable logical working directory, independent of temporary session
    /// files.
    pub root: Utf8PathBuf,
    /// JP's in-process MCP endpoint, with policy controlled through Host
    /// channels.
    pub mcp_endpoint: Option<Url>,
    /// Host identity for scoping derived agent files.
    /// Auxiliary requests that have no conversation owner leave this unset.
    pub invocation: Option<InvocationContext>,
}

/// Who dispatches the tool calls reported in a provider stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolExecution {
    /// JP submits each model-requested call through its MCP connection.
    #[default]
    Caller,
    /// The external agent submits calls; JP controls their pending
    /// interactions.
    Agent {
        /// MCP metadata field that carries the provider's tool-call identifier.
        correlation_key: &'static str,
    },
}

/// A provider stream and its tool-dispatch contract.
pub struct QueryStream {
    /// Events for the response, retained across tool phases for an agent loop.
    pub events: EventStream,
    /// Whether tool calls are submitted by JP or by the external agent.
    pub execution: ToolExecution,
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
