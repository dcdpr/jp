//! Shared rendering components for conversation content.
//!
//! These renderers are used by both the live-stream query pipeline and the
//! conversation print/replay path.

pub(crate) mod chat;
pub(crate) mod metadata;
pub(crate) mod structured;
pub(crate) mod tool;
pub(crate) mod turn;

pub(crate) use chat::ChatRenderer;
pub(crate) use structured::StructuredRenderer;
pub(crate) use tool::ToolRenderer;
pub(crate) use turn::{ConfigSource, TurnRenderer};
