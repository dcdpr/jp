//! Inline terminal prompt widgets (reply and select) for JP.

mod inline_reply;
mod inline_select;
pub mod prompt;

pub use inline_reply::{InlineReply, ReplyEditMode, ReplyOutcome};
pub use inline_select::{InlineOption, InlineSelect};
