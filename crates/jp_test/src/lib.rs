use std::{env, path::PathBuf};

pub mod macros;
pub mod mock;

pub type Result = std::result::Result<(), Box<dyn std::error::Error>>;
pub use test_log::test;

/// Absolute path to the calling package's `tests/fixtures` directory.
///
/// Read from the `CARGO_MANIFEST_DIR` environment variable, which both `cargo
/// test` and `cargo nextest` set for the process running a test, keyed to the
/// package that test belongs to.
///
/// Reading it at runtime rather than through the compile-time macro keeps the
/// path correct when a test binary is shared between git worktrees through a
/// common target directory: the macro records whichever worktree built the
/// binary, which is not necessarily the one running it.
///
/// # Panics
///
/// Panics if the variable is unset, which happens when a test binary is
/// launched directly instead of through cargo or nextest.
/// Failing here beats returning a path that quietly resolves outside the
/// package.
#[must_use]
pub fn fixtures_dir() -> PathBuf {
    let root = env::var_os("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR: run tests via `cargo test` or `cargo nextest`");

    PathBuf::from(root).join("tests/fixtures")
}
