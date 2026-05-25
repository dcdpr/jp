//! CLI argument definitions.
//!
//! Two binaries point at the same `main.rs`: `comfort` (direct) and
//! `cargo-comfort` (a cargo subcommand).
//! The binary entry detects which one it was invoked as, strips the leading
//! `comfort` argv inserted by cargo, and adjusts defaults — direct invocation
//! defaults to stdin/stdout, cargo invocation defaults to `--workspace`.

use std::path::PathBuf;

use clap::Parser;

use crate::DEFAULT_MAX_WIDTH;

/// Source language to format.
/// With [`Auto`], per-file detection (extension or `--stdin-filename`)
/// determines the format and workspace/directory walks include both Rust and
/// Markdown files.
/// With an explicit language, every selected file is formatted as that language
/// and walks filter to its extensions only.
///
/// [`Auto`]: Language::Auto
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
#[clap(rename_all = "kebab-case")]
pub enum Language {
    /// Detect per file: `.rs` → Rust, `.md`/`.markdown` → Markdown,
    /// everything else → Rust (the stdin default and the dominant use case).
    #[default]
    Auto,
    /// Force Rust mode regardless of extension.
    Rust,
    /// Force Markdown mode regardless of extension.
    Markdown,
}

impl Language {
    /// Resolve the effective format for a given file path.
    /// `None` for `path` means the caller has no filename hint (e.g. stdin
    /// without `--stdin-filename`), in which case `Auto` defaults to Rust.
    #[must_use]
    pub fn resolve(self, path: Option<&std::path::Path>) -> Format {
        match self {
            Self::Rust => Format::Rust,
            Self::Markdown => Format::Markdown,
            Self::Auto => match path.and_then(|p| p.extension()).and_then(|e| e.to_str()) {
                Some("md" | "markdown") => Format::Markdown,
                _ => Format::Rust,
            },
        }
    }
}

/// Resolved per-file format used by [`run`] to dispatch to the correct
/// pipeline.
///
/// [`run`]: crate::run::run
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Rust,
    Markdown,
}

#[cfg(test)]
#[path = "cli_tests.rs"]
mod tests;

/// How the binary was invoked.
/// Determines whether the empty-args default is stdin (direct) or `--workspace`
/// (cargo subcommand).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Invocation {
    Direct,
    Cargo,
}

#[derive(Debug, Parser)]
#[command(
    name = "comfort",
    about = "Format Rust doc comments with semantic line breaks.",
    long_about = "Reflows outer (`///`) and inner (`//!`) doc-comment blocks using semantic line \
                  breaks (one sentence per line), with an optional `--max-width` safety net. \
                  Inline `//` comments and `/** */` block-style doc comments are left untouched.",
    version
)]
pub struct Cli {
    /// Files or directories to format.
    /// Directories are walked recursively; `.gitignore` is honored.
    /// If no paths are given, the tool reads from stdin and writes to stdout
    /// (direct invocation) or walks the whole workspace (cargo subcommand).
    ///
    /// Mutually exclusive with `--workspace`, `--package`, and `--exclude`.
    #[arg(conflicts_with_all = ["workspace", "packages", "exclude"])]
    pub paths: Vec<PathBuf>,

    /// Format every `.rs` file under the current cargo workspace.
    /// Default for `cargo comfort`; explicit for `comfort`.
    #[arg(long)]
    pub workspace: bool,

    /// Limit the workspace walk to the named package(s).
    /// Repeat the flag for multiple packages.
    /// Implies workspace mode.
    #[arg(short = 'p', long = "package", value_name = "SPEC")]
    pub packages: Vec<String>,

    /// Exclude the named package(s) from the workspace walk.
    /// Repeat the flag for multiple packages.
    /// Implies workspace mode.
    #[arg(long = "exclude", value_name = "SPEC")]
    pub exclude: Vec<String>,

    /// Check whether files would change; print a diff and exit non-zero if any
    /// do.
    /// Never writes to disk.
    #[arg(long)]
    pub check: bool,

    /// Print the path of each changed file to stdout, one per line.
    /// In write mode, lists files that were reformatted; in `--check` mode,
    /// lists files that would be reformatted (and suppresses the diff).
    #[arg(long)]
    pub list_changed: bool,

    /// Force a specific source language.
    /// With `auto` (default), detect from each file's extension and let
    /// workspace/directory walks pick up both Rust and Markdown.
    /// With `rust` or `markdown`, every selected file is formatted in that mode
    /// and walks filter to its extensions only.
    #[arg(long, value_enum, default_value_t = Language::Auto)]
    pub language: Language,

    /// Also canonicalize the markdown structure of each formatted body: align
    /// tables, normalise list markers, prefer fenced over indented code blocks,
    /// etc. Off by default — in default mode, only paragraph prose gets
    /// reflowed and everything else is preserved byte-for-byte.
    #[arg(long)]
    pub format_markdown: bool,

    /// Convert inline markdown links to reference-style links and move all
    /// reference definitions to the bottom of the body.
    /// Adaptive: shortcut form `[text]` where possible, full form
    /// `[text][label]` for collisions.
    /// Independent of `--format-markdown` — enable either, both, or neither.
    #[arg(long)]
    pub reference_links: bool,

    /// Maximum line width for reflow.
    /// Long sentences wrap at word boundaries within sembr blocks.
    /// `0` disables width wrapping.
    #[arg(long, default_value_t = DEFAULT_MAX_WIDTH)]
    pub max_width: usize,

    /// The original filename for content piped via stdin.
    /// In `--language auto` (default), the extension drives format detection —
    /// e.g. `--stdin-filename notes.md` switches to Markdown mode.
    /// Also improves diagnostic messages; defaults to `<stdin>`.
    #[arg(long, value_name = "PATH")]
    pub stdin_filename: Option<PathBuf>,
}
