//! Conversation-specific types and utilities.

#![warn(
    clippy::all,
    clippy::allow_attributes,
    clippy::cargo,
    clippy::missing_docs_in_private_items,
    clippy::nursery,
    clippy::pedantic,
    clippy::renamed_function_params,
    clippy::tests_outside_test_module,
    clippy::todo,
    clippy::try_err,
    clippy::unimplemented,
    clippy::unneeded_field_pattern,
    clippy::unseparated_literal_suffix,
    clippy::unused_result_ok,
    clippy::unused_trait_names,
    clippy::use_debug,
    clippy::unwrap_used,
    missing_docs,
    rustdoc::all,
    unused_doc_comments
)]
#![expect(
    clippy::multiple_crate_versions,
    reason = "we need to update rmcp to update base64"
)]

pub mod conversation;
pub mod error;
pub mod event;
pub mod stream;
pub mod thread;

pub use conversation::{Conversation, ConversationId, ConversationsMetadata};
pub use error::Error;
pub use stream::ConversationStream;
