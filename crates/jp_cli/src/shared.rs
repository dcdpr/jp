//! Shared helpers reached for by multiple subcommands.
//!
//! Anything in here is *command-supporting infrastructure* — utilities
//! commands import and use — as distinct from the crate-root spine (`ctx`,
//! `output`, `parser`, `session`, …) which is what `lib.rs` plumbs together to
//! make the CLI run.
//!
//! Inclusion test: a module belongs here when it's used by more than one
//! subcommand and isn't part of the bootstrap path.

pub(crate) mod confirm;
pub(crate) mod search;
