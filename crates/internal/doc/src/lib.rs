//! The parts of the document format JP's in-repo documents share.
//!
//! Each kind of document keeps its own format and commands, RFDs in `jp-rfd`
//! and tickets in `ticket`; what any kind can reuse lives here.
//!
//! - [`metadata`] reads and edits the `- **Key**: Value` block under a
//!   document's title.
//! - [`id`] defines the seven-character ids documents like tickets are named
//!   by, and [`epoch`] the day a directory's ids count time from.
//!   [`files`] allocates ids and finds the file each one names.
//! - [`references`] rewrites what other documents say about one whose id or
//!   file name changed, and finds the mentions a rewrite didn't reach.
//! - [`heading`] reads a document's headings and the anchors the site gives
//!   them.
//! - [`repository`] reads the web address links into the repository point at.
//!
//! See `docs/rfd/127-in-repo-project-wiki.md`.

pub mod epoch;
pub mod files;
pub mod heading;
pub mod id;
pub mod metadata;
pub mod references;
pub mod repository;
