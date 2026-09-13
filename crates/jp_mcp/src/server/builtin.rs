//! Builtin tool trait and executor registry.
//!
//! Maps tool names to their Rust implementations.

pub mod describe_tools;

use std::{collections::HashMap, sync::Arc};

use async_trait::async_trait;
use indexmap::IndexMap;
use jp_tool::Outcome;
use serde_json::Value;

/// A built-in tool that executes Rust code instead of shelling out.
#[async_trait]
pub trait BuiltinTool: Send + Sync {
    /// Execute the tool with the given arguments and accumulated answers.
    async fn execute(&self, arguments: &Value, answers: &IndexMap<String, Value>) -> Outcome;
}

/// Registry mapping builtin tool names to their executors.
#[derive(Clone, Default)]
pub struct BuiltinExecutors {
    executors: HashMap<String, Arc<dyn BuiltinTool>>,
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
    pub fn get(&self, name: &str) -> Option<Arc<dyn BuiltinTool>> {
        self.executors.get(name).cloned()
    }
}
