//! Check that a workspace keeps its test code in test files.
//!
//! A production file reaches its tests through one declaration, its last item:
//! `#[cfg(test)] #[path = "foo_tests.rs"] mod tests;` for a module with one
//! test module, or `#[cfg(test)] mod tests;`, which loads `foo/tests.rs`, for a
//! module with several.
//! No other code in a production file depends on `cfg(test)`, and `#[test]`
//! functions live only in files named `tests.rs` or `*_tests.rs`.
//!
//! [`check_workspace`] walks the library, binary, and proc-macro targets of
//! every workspace member from their crate roots, following `mod` declarations
//! the way rustc resolves them, and reports each breach with its file and line.
//! [`check`] is the same walk over any [`Sources`].

mod cfg;
mod layout;
mod scan;
mod tree;

use std::{collections::BTreeSet, fmt, fs, io};

use camino::{Utf8Path, Utf8PathBuf};
use cargo_metadata::{MetadataCommand, TargetKind};
pub use tree::check;

/// Where the walk reads source files from.
pub trait Sources {
    /// The contents of the file at `path`.
    ///
    /// # Errors
    ///
    /// Returns the error reading the file produced.
    fn read(&self, path: &Utf8Path) -> io::Result<String>;

    /// Whether a file exists at `path`.
    fn exists(&self, path: &Utf8Path) -> bool;
}

/// Source files on disk, under `root`.
///
/// A relative path is read relative to `root`, so the walk and the violations
/// it reports can name files relative to it.
pub struct Disk {
    pub root: Utf8PathBuf,
}

impl Sources for Disk {
    fn read(&self, path: &Utf8Path) -> io::Result<String> {
        fs::read_to_string(self.root.join(path))
    }

    fn exists(&self, path: &Utf8Path) -> bool {
        self.root.join(path).is_file()
    }
}

/// One breach of the layout, in the file and on the line it occurs.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Violation {
    pub file: Utf8PathBuf,
    pub line: usize,
    pub breach: Breach,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.file, self.line, self.breach)
    }
}

/// What a [`Violation`] breaks.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, thiserror::Error)]
pub enum Breach {
    /// Code compiled only for tests, in a production file.
    #[error(
        "test-only {what}: move it into a test file, or inject the dependency it stands in for"
    )]
    TestOnly { what: &'static str },

    /// A `cfg(not(test))`, which makes production code differ from what the
    /// tests run.
    #[error(
        "`cfg(not(test))`: inject the dependency instead of compiling another version for tests"
    )]
    NotTest,

    /// `#[test]` functions in a file not named `tests.rs` or `*_tests.rs`.
    #[error("`#[test]` functions outside a `tests.rs` or `*_tests.rs` file ({count} here)")]
    TestFunctions { count: usize },

    /// A `mod tests { … }` block in a production file.
    #[error("inline test module: move its body into the module's test file")]
    InlineTestModule,

    /// A `mod tests;` declaration with items after it.
    #[error("the `mod tests;` declaration must be the file's last item")]
    DeclarationNotLast,

    /// A test module other than `tests`, declared from a production file.
    #[error(
        "test module `{name}`: a production file declares only `mod tests;`, so declare `{name}` \
         from the file that loads"
    )]
    ExtraTestModule { name: String },

    /// A `mod tests;` declaration loading a file other than the module's test
    /// file.
    ///
    /// `None` stands for a declaration without a `#[path]`.
    #[error(
        "test module declared with {}; expected {}",
        spelling(found.as_deref()),
        alternatives(expected)
    )]
    TestModulePath {
        found: Option<String>,
        expected: Vec<Option<String>>,
    },

    /// A `mod.rs` file.
    #[error("`mod.rs` file: move it to `{rename}`")]
    ModRs { rename: Utf8PathBuf },

    /// A `mod name;` declaration in a test file without a `#[path]`.
    #[error("`mod {name};` in a test file needs a `#[path]` naming a file in the same directory")]
    UnpathedTestModule { name: String },

    /// A `#[path]` in a test file naming a file in another directory.
    #[error("`#[path = \"{path}\"]` in a test file must name a file in the same directory")]
    NotSibling { path: String },

    /// A `*_tests.rs` file declaring further test modules.
    #[error(
        "a `*_tests.rs` file is a module's only test module, so it declares none: move this file \
         to `{move_to}`"
    )]
    SingleFileDeclaresModules { move_to: Utf8PathBuf },

    /// A `tests.rs` file declaring no other test module.
    #[error("declares no other test module: rename this file to `{rename}`")]
    DirectoryWithOneModule { rename: Utf8PathBuf },

    /// A `mod name;` declaration whose file does not exist.
    #[error("no file found for module `{name}`")]
    MissingModule { name: String },

    /// A file that could not be read.
    #[error("cannot read: {error}")]
    Unreadable { error: String },

    /// A file that is not valid Rust.
    #[error("not valid Rust: {error}")]
    Unparsable { error: String },
}

/// How a test module declaration is spelled: with the `#[path]` it carries, or
/// without one.
fn spelling(path: Option<&str>) -> String {
    match path {
        Some(path) => format!("`#[path = \"{path}\"]`"),
        None => "no `#[path]`".to_owned(),
    }
}

fn alternatives(spellings: &[Option<String>]) -> String {
    spellings
        .iter()
        .map(|path| spelling(path.as_deref()))
        .collect::<Vec<_>>()
        .join(" or ")
}

/// Why the check could not run.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cargo metadata: {0}")]
    Metadata(#[from] cargo_metadata::Error),
}

/// Check every workspace member of the workspace holding the current directory.
///
/// Violations name files relative to the workspace root.
///
/// # Errors
///
/// Returns an error when `cargo metadata` fails.
pub fn check_workspace() -> Result<Vec<Violation>, Error> {
    let metadata = MetadataCommand::new().no_deps().exec()?;
    let root = metadata.workspace_root.clone();

    let roots: BTreeSet<Utf8PathBuf> = metadata
        .workspace_packages()
        .into_iter()
        .flat_map(|package| &package.targets)
        .filter(|target| target.kind.iter().any(is_production_target))
        .map(|target| {
            target
                .src_path
                .strip_prefix(&root)
                .map_or_else(|_| target.src_path.clone(), ToOwned::to_owned)
        })
        .collect();

    Ok(check(roots, &Disk { root }))
}

/// Whether a target compiles code a production build ships.
///
/// Integration tests, benchmarks, and examples are test or sample code
/// throughout, and build scripts never compile with `cfg(test)`.
fn is_production_target(kind: &TargetKind) -> bool {
    matches!(
        kind,
        TargetKind::Lib
            | TargetKind::Bin
            | TargetKind::ProcMacro
            | TargetKind::RLib
            | TargetKind::DyLib
            | TargetKind::CDyLib
            | TargetKind::StaticLib
    )
}
