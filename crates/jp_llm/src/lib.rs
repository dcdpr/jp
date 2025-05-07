mod error;
pub mod provider;
mod structured;

pub use error::Error;
pub use provider::{CompletionChunk, Provider};
pub use structured::completion as structured_completion;
