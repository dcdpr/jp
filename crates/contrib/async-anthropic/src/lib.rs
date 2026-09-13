#![allow(
    rustdoc::private_intra_doc_links,
    reason = "we don't host the docs, and use them mainly for LSP integration"
)]

pub mod bearer;
mod client;
pub mod errors;
pub mod messages;
pub mod models;
pub mod types;
pub use client::Client;
