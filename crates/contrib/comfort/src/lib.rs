//! Semantic line-break formatter for Rust doc comments.
//!
//! `comfort` walks Rust source files, locates outer (`///`) and inner (`//!`)
//! doc-comment blocks, and reflows each block's prose paragraphs with semantic
//! line breaks (one sentence per line) plus an optional `max_width` safety net.
//!
//! Non-doc code, inline `//` comments, and `/** */` block-style doc comments
//! are left untouched.
//! Markdown structure inside doc comments — reference link definitions, block
//! quotes, lists, code blocks, headings, tables — is preserved verbatim; only
//! paragraph contents are reflowed.
//!
//! The library formats text: [`format`] holds the entry points, and [`config`]
//! reads the style a `.comfort.toml` file sets.
//! The `cli` feature, on by default, adds what the `comfort` and
//! `cargo-comfort` binaries need on top: argument parsing, the workspace walk,
//! and `--check` diffs.

#[cfg(feature = "cli")]
pub mod cli;
pub mod config;
pub mod extract;
pub mod format;
#[cfg(feature = "cli")]
pub mod run;
pub mod sentence;
#[cfg(feature = "cli")]
pub mod walk;

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;

use std::path::PathBuf;
#[cfg(feature = "cli")]
use std::{ffi::OsString, path::Path, process::ExitCode};

#[cfg(feature = "cli")]
use clap::Parser;

#[cfg(feature = "cli")]
use crate::cli::{Cli, Invocation};

/// Default maximum line width for wrapped doc-comment content.
pub const DEFAULT_MAX_WIDTH: usize = 80;

/// Shared binary entry-point.
/// Both `comfort` and `cargo-comfort` delegate here; the invocation mode is
/// detected from `argv[0]` at runtime.
///
/// `eprintln!` is otherwise denied by the workspace lints — allowing it here
/// keeps fatal-error reporting in one place.
#[cfg(feature = "cli")]
#[allow(clippy::print_stderr)]
#[must_use]
pub fn cli_main() -> ExitCode {
    let raw: Vec<OsString> = std::env::args_os().collect();
    let (invocation, args) = parse_invocation(raw);

    let cli = Cli::parse_from(args);

    match run::run(&cli, invocation) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Error::CheckFailed(_)) => ExitCode::from(1),
        Err(err) => {
            eprintln!("comfort: {err}");
            ExitCode::from(2)
        }
    }
}

/// Identify whether we were invoked directly (`comfort`) or by cargo
/// (`cargo-comfort`).
/// For the cargo case, cargo passes the subcommand name (`comfort`) as
/// `args[1]`, which we strip before handing args to clap.
#[cfg(feature = "cli")]
fn parse_invocation(mut raw: Vec<OsString>) -> (Invocation, Vec<OsString>) {
    let Some(bin) = raw
        .first()
        .and_then(|p| Path::new(p).file_name().map(OsString::from))
    else {
        return (Invocation::Direct, raw);
    };

    // On Windows the binary name carries `.exe`; match either form.
    let is_cargo = bin == *"cargo-comfort" || bin == *"cargo-comfort.exe";

    if !is_cargo {
        return (Invocation::Direct, raw);
    }

    // Cargo always passes the subcommand name as args[1]. Skip it if present.
    if raw.get(1).is_some_and(|s| s == "comfort") {
        raw.remove(1);
    }
    (Invocation::Cargo, raw)
}

/// Errors produced by the comfort library.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[cfg(feature = "cli")]
    #[error("cargo metadata: {0}")]
    CargoMetadata(#[from] cargo_metadata::Error),

    #[cfg(feature = "cli")]
    #[error("walk: {0}")]
    Walk(#[from] ignore::Error),

    /// Failed to read a source or configuration file.
    /// Carries the path so the user knows which file failed when walking many
    /// at once.
    #[error("failed to read {path}: {source}")]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// A `.comfort.toml` that doesn't hold a valid configuration.
    #[error("invalid configuration in {path}: {source}")]
    Config {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    /// Failed to write a reformatted file back to disk.
    #[cfg(feature = "cli")]
    #[error("failed to write {path}: {source}")]
    WriteFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// One of the names passed to `-p`/`--package` or `--exclude` doesn't match
    /// any workspace package.
    #[cfg(feature = "cli")]
    #[error("unknown package: {0}")]
    UnknownPackage(String),

    /// Reported in `--check` mode when at least one file would be reformatted.
    /// Carries the count of files that differ.
    #[cfg(feature = "cli")]
    #[error("{0} file(s) would be reformatted")]
    CheckFailed(usize),
}
