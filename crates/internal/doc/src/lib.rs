//! Shared storage, metadata, and ID primitives for in-repo documents.
//!
//! Each document kind defines its own schema and commands (`jp-rfd` for RFDs,
//! `ticket` for tickets, `jp-wiki` for wiki pages); this crate provides the
//! shared building blocks:
//!
//! - [`metadata`] parses and updates the `- **Key**: Value` block below a
//!   document's title, and [`author`] resolves the author identity from JP or
//!   git configuration.
//! - [`id`] defines the 7-character base32 document IDs, [`epoch`] manages the
//!   per-directory start date from which ID time buckets are measured, and
//!   [`files`] allocates IDs and locates document files or directories.
//! - [`references`] rewrites cross-references when a document's ID or filename
//!   changes, and reports stale references.
//! - [`heading`] extracts Markdown headings and their generated HTML anchors.
//! - [`lock`] acquires advisory file locks to serialize concurrent writers.
//! - [`repository`] resolves the repository web URL for document links.
//! - [`format`] formats Markdown documents on write when enabled in workspace
//!   configuration.
//!
//! See `docs/rfd/127-in-repo-project-wiki.md`.
//!
//! [`format`]: crate::format

pub mod author;
pub mod epoch;
pub mod files;
pub mod format;
pub mod heading;
pub mod id;
pub mod lock;
pub mod metadata;
pub mod references;
pub mod repository;
