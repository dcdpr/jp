//! Parsing, validation, and release-note rendering for changelog fragments and
//! highlights.
//!
//! A project stores fragments and highlights in a single directory
//! (`changelog.d/` by default):
//!
//! - A **fragment** (`<slug>.<type>.md`) records a single user-visible change
//!   under its [`Type`], along with an optional [`Upgrade`] section for
//!   breaking changes.
//! - A **highlight** (`highlights/<slug>.md`) is an editorial section of the
//!   release notes for a major feature, containing a `# Title`, `Authors`
//!   metadata, and prose.
//!
//! [`fragment::parse`] and [`highlight::parse`] parse and validate individual
//! files.
//! [`check()`] validates the directory layout and every fragment and highlight
//! in it, including released subdirectories.
//!
//! [`release::read`] assembles a [`Release`] from a release subdirectory (or
//! from all unreleased files), attributing each file to the change requests or
//! commits that modified it in git history.
//! A [`Release`] serializes to JSON and renders to Markdown release notes and
//! upgrade guides via [`markdown`].
//!
//! [`cli`] implements the standalone `changelog` binary used in CI.
//! This crate does not read workspace configuration; callers pass
//! project-specific settings as arguments.
//!
//! See `docs/rfd/121-changelog-fragments-and-validation.md` and
//! `docs/rfd/122-release-pipeline.md` for the format and pipeline
//! specifications.

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

pub use directory::{CheckError, Place, Problem, ProblemKind, Report, check};
pub use document::{ListedCredit, with_credits};
pub use forge::{Credit, CreditKind, Forge, NoAddress, UnknownForge};
pub use fragment::{Fragment, FragmentError, NameError, Type, UnknownType, Upgrade};
pub use git::GitError;
pub use highlight::{Highlight, HighlightError};
pub use history::{Added, History};
pub use release::{Headings, Release, ReleaseError};
