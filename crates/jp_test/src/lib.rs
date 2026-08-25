use std::{env, path::PathBuf};

pub mod macros;
pub mod mock;

pub type Result = std::result::Result<(), Box<dyn std::error::Error>>;
pub use test_log::test;

/// Absolute path to the calling package's `tests/fixtures` directory.
///
/// Derived from the process working directory, which both `cargo test` and
/// `cargo nextest` set to the package root.
///
/// Resolving at runtime rather than baking in `CARGO_MANIFEST_DIR` keeps the
/// path correct when a compiled test binary is shared between git worktrees
/// through a common target directory: a compile-time path records whichever
/// worktree built the binary, which is not necessarily the one running it.
///
/// Callers must not change the working directory before calling this.
///
/// # Panics
///
/// Panics if the working directory cannot be read.
#[must_use]
pub fn fixtures_dir() -> PathBuf {
    env::current_dir()
        .expect("readable working directory")
        .join("tests/fixtures")
}
