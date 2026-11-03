//! Changelog fragments: the entries a project records, one per change its users
//! notice, to build its release notes from.
//!
//! A project keeps its fragments in one directory, `changelog.d/` by default.
//! Each fragment is a Markdown file whose frontmatter names the change's
//! [`Type`], followed by the entry the release notes list and, for a change
//! that requires users to act, its section of the release's upgrade guide.
//!
//! [`fragment::parse`] reads one fragment and holds it to the format.
//! [`check()`] holds a whole fragments directory to its layout, released
//! fragments included, and every fragment in it to the format.
//! The `changelog` binary runs [`check()`] for CI, through [`cli`].
//!
//! The crate reads no configuration: whatever differs between projects, such as
//! where the fragments directory is, arrives as an argument.
//!
//! The format is specified in
//! `docs/rfd/121-changelog-fragments-and-validation.md`.

pub mod cli;
pub mod directory;
pub mod fragment;

pub use directory::{CheckError, Problem, ProblemKind, Report, check};
pub use fragment::{Fragment, FragmentError, Type, Upgrade};
