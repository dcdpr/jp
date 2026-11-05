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
//!
//! [`release::read`] builds a [`Release`] from a release's directory, or from
//! the files no release has taken yet, crediting each file to the change
//! requests that wrote it, which it reads from git's history.
//! A release serializes to JSON, and [`markdown`] renders its release notes and
//! upgrade guide.
//!
//! The `changelog` binary runs [`check()`] for CI, and renders releases,
//! through [`cli`].
//!
//! The crate reads no configuration: whatever differs between projects, such as
//! where the fragments directory is, arrives as an argument.
//!
//! The formats are specified in
//! `docs/rfd/121-changelog-fragments-and-validation.md`, and releases in
//! `docs/rfd/122-release-pipeline.md`.

pub mod cli;
pub mod directory;
mod document;
pub mod forge;
pub mod fragment;
mod git;
pub mod highlight;
mod history;
mod links;
pub mod markdown;
pub mod release;

pub use directory::{CheckError, Problem, ProblemKind, Report, check, reserved_for};
pub use document::FrontmatterError;
pub use forge::{Credit, CreditKind, Forge, UnknownForge};
pub use fragment::{Fragment, FragmentError, Type, UnknownType, Upgrade};
pub use git::GitError;
pub use highlight::{Highlight, HighlightError};
pub use release::{Headings, Release, ReleaseError};
