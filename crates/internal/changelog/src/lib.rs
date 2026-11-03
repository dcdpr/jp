//! Changelog fragments and highlights: what a project records about its changes
//! for its users, to build its release notes from.
//!
//! A project keeps them in one directory, `changelog.d/` by default.
//! A fragment is one entry per change users notice: a Markdown file whose
//! frontmatter names the change's [`Type`], followed by the entry the release
//! notes list and, for a change that requires users to act, its section of the
//! release's upgrade guide.
//! A highlight, in the directory's `highlights/`, is an editorial section of
//! the release notes, for a feature worth more than its entry.
//!
//! [`fragment::parse`] and [`highlight::parse`] read one file and hold it to
//! its format.
//! [`check()`] holds a whole fragments directory to its layout, released files
//! included, and every file in it to its format.
//! The `changelog` binary runs [`check()`] for CI, through [`cli`].
//!
//! The crate reads no configuration: whatever differs between projects, such as
//! where the fragments directory is, arrives as an argument.
//!
//! The formats are specified in
//! `docs/rfd/121-changelog-fragments-and-validation.md`.

pub mod cli;
pub mod directory;
mod document;
pub mod fragment;
pub mod highlight;

pub use directory::{CheckError, Problem, ProblemKind, Report, check};
pub use document::FrontmatterError;
pub use fragment::{Fragment, FragmentError, Type, Upgrade};
pub use highlight::{Highlight, HighlightError};
