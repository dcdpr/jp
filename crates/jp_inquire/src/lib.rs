//! Inline terminal prompt widgets (reply and select) for JP.

mod close;
mod inline_reply;
mod inline_select;
pub mod prompt;
mod terminal;

pub use close::{PromptCloser, close_all, close_requested, closing_on};
pub use inline_reply::{InlineReply, ReplyEditMode, ReplyOutcome};
pub use inline_select::{InlineOption, InlineSelect};
pub use terminal::{release_terminal, terminal_held};
