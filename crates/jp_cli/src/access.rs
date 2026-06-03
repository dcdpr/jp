//! Host-side tool access-grant machinery.
//!
//! This module owns the pieces JP runs before a tool spawns: parsing the
//! `--mount` flag, resolving and approving external symlink targets, and
//! compiling `access.fs` config into the [`jp_tool::AccessPolicy`] the tool
//! receives in its context.
//!
//! - [`mount`] parses `[TOOL:]NAME=PATH[:MODE]` specs and builds the config
//!   rules a mount injects.
//! - [`approvals`] is the user-local trust-on-first-use store binding a mount
//!   path to a canonical target.
//! - [`compile`] turns merged `access.fs` config into a compiled policy, baking
//!   approved external targets into each rule.

pub(crate) mod approvals;
pub(crate) mod compile;
pub(crate) mod mount;
