//! Builtin tool trait and executor registry.
//!
//! Maps tool names to their Rust implementations.

pub mod describe_tools;

use std::{collections::HashMap, sync::Arc};

use futures::future::BoxFuture;
use indexmap::IndexMap;
use jp_tool::Outcome;
use serde_json::Value;

/// A built-in tool that executes Rust code instead of shelling out.
///
/// The return type is [`Outcome`], so a built-in produces text, an error, or a
/// question — the same three shapes a local command can print on stdout.
/// A resource, an image, structured content, or MCP annotations are not
/// reachable from here; only a tool on an upstream MCP server can return those,
/// because only that path carries a native MCP result into [`ToolResult`].
/// Widening this is [RFD 058]'s work, not something to route around one tool at
/// a time.
///
/// [RFD 058]: https://jp.computer/rfd/058-typed-content-blocks-for-tool-responses
/// [`ToolResult`]: jp_tool::ToolResult
pub trait BuiltinTool: Send + Sync {
    /// Execute the tool with the given arguments and accumulated answers.
    fn execute(
        &self,
        arguments: &Value,
        answers: &IndexMap<String, Value>,
    ) -> impl Future<Output = Outcome> + Send;
}

/// A [`BuiltinTool`] behind a pointer: `Arc<dyn DynBuiltinTool>`.
///
/// Every `BuiltinTool` implements this trait, and [`execute`] does what
/// [`BuiltinTool::execute`] does, returning a boxed future.
/// Implement [`BuiltinTool`], not this trait.
///
/// [`execute`]: Self::execute
pub trait DynBuiltinTool: Send + Sync {
    fn execute<'a>(
        &'a self,
        arguments: &'a Value,
        answers: &'a IndexMap<String, Value>,
    ) -> BoxFuture<'a, Outcome>;
}

impl<T: BuiltinTool> DynBuiltinTool for T {
    fn execute<'a>(
        &'a self,
        arguments: &'a Value,
        answers: &'a IndexMap<String, Value>,
    ) -> BoxFuture<'a, Outcome> {
        Box::pin(BuiltinTool::execute(self, arguments, answers))
    }
}

/// Registry mapping builtin tool names to their executors.
#[derive(Clone, Default)]
pub struct BuiltinExecutors {
    executors: HashMap<String, Arc<dyn DynBuiltinTool>>,
}

impl BuiltinExecutors {
    #[must_use]
    pub fn new() -> Self {
        Self {
            executors: HashMap::new(),
        }
    }

    #[must_use]
    pub fn register(mut self, name: impl Into<String>, tool: impl BuiltinTool + 'static) -> Self {
        self.executors.insert(name.into(), Arc::new(tool));
        self
    }

    #[must_use]
    pub fn get(&self, name: &str) -> Option<Arc<dyn DynBuiltinTool>> {
        self.executors.get(name).cloned()
    }
}
