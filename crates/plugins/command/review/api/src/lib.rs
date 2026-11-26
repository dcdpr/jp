//! The request and response types of the JSON HTTP API `jp review serve`
//! answers.
//!
//! The server serializes these types and a browser client deserializes the same
//! ones, so the two can't disagree on a field name or an enum variant.
//! The crate holds plain data and `serde` derives only: no I/O, nothing that
//! keeps it from compiling for `wasm32-unknown-unknown`.
//!
//! - [`request`] holds the bodies the server takes, and the query parameters of
//!   its `GET` endpoints.
//! - [`response`] holds what the server answers, and the vocabulary a review is
//!   described in: statuses, roles, verdicts, and how a pass ended.
//!
//! Identifiers travel as the text `jp review` prints: `R-1h4k2m9` for a review,
//! `R-1h4k2m9#2` for a comment, and `jp-c17629384710` for a conversation.
//! Ranges travel as `--at` writes them after the path, `42`, `42-45`, or
//! `42:5-43:18`, and as half-open UTF-8 byte offsets into one version of a
//! file.
//!
//! See `docs/rfd/140-browser-frontend-for-the-review-engine.md`.

pub mod request;
pub mod response;

pub use crate::{request::*, response::*};
