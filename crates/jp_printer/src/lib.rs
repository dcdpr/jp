//! The configuration types for Jean-Pierre.

#![warn(
    clippy::all,
    clippy::allow_attributes,
    // clippy::cargo,
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
#![allow(
    rustdoc::private_intra_doc_links,
    reason = "we don't host the docs, and use them mainly for LSP integration"
)]

mod printer;
mod typewriter;

pub use printer::*;
