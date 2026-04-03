//! A format-preserving JSON and JSON5 editor.
//!
//! `json_edit` parses JSON or JSON5 text into a lossless syntax tree (powered
//! by [`rowan`]) and exposes high-level editing operations that touch only the
//! modified keys. Comments, whitespace, key order, and quote styles are
//! preserved for everything that wasn't explicitly changed.
//!
//! # Quick start
//!
//! ```
//! let doc = json_edit::Document::parse(r#"{"greeting": "hello"}"#).unwrap();
//! let obj = doc.as_object().unwrap();
//! obj.set("greeting", "\"world\"");
//! assert_eq!(doc.to_string(), r#"{"greeting": "world"}"#);
//! ```

pub mod ast;
pub mod error;
pub mod lexer;
pub mod merge;
pub mod parser;
pub mod syntax;

pub use ast::Document;
pub use error::{MergeError, ParseError};
pub use lexer::Dialect;
pub use merge::deep_merge;
