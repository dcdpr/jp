//! Where rustc finds a module's file, and where a module's tests belong.

use camino::{Utf8Component, Utf8Path, Utf8PathBuf};

/// How the tests a file reaches are laid out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TestForm {
    /// `foo_tests.rs`: the module's only test module.
    ///
    /// `directory` is where it moves once the module has several.
    Single { directory: Utf8PathBuf },

    /// `foo/tests.rs`: the first of several test modules.
    ///
    /// `single` is where it moves once the module has only one.
    Directory { single: Utf8PathBuf },

    /// Any other test file, declared from a test file or from a misplaced
    /// declaration.
    Nested,
}

/// A `mod name;` declaration, without a body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Declaration {
    pub(crate) name: String,
    pub(crate) line: usize,

    /// The `#[path]` value, if it carries one.
    pub(crate) path: Option<String>,

    /// The inline `mod name { … }` blocks around the declaration, outermost
    /// first.
    pub(crate) inline: Vec<String>,
}

/// The files `decl` in `file` may load, in the order rustc tries them, each
/// with whether the loaded file owns its directory.
///
/// A file owns its directory when it is a crate root, a `mod.rs`, or loaded
/// through a `#[path]`: its child modules then live beside it rather than in a
/// directory named after it.
pub(crate) fn candidates(
    file: &Utf8Path,
    owns_dir: bool,
    decl: &Declaration,
) -> Vec<(Utf8PathBuf, bool)> {
    let file_dir = file.parent().unwrap_or(Utf8Path::new("")).to_owned();
    let mut module_dir = if owns_dir {
        file_dir.clone()
    } else {
        file.with_extension("")
    };
    module_dir.extend(&decl.inline);

    match &decl.path {
        // At a file's top level a `#[path]` is relative to the file's own
        // directory, and inside an inline block to that block's directory.
        Some(path) => {
            let base = if decl.inline.is_empty() {
                file_dir
            } else {
                module_dir
            };
            vec![(normalize(&base.join(path)), true)]
        }
        None => vec![
            (module_dir.join(format!("{}.rs", decl.name)), false),
            (module_dir.join(&decl.name).join("mod.rs"), true),
        ],
    }
}

/// The [`TestForm`] a `mod tests;` declaration in `file` sets up, given the
/// `#[path]` it carries.
///
/// # Errors
///
/// Returns the spellings the declaration may use instead, `None` standing for
/// no `#[path]`, when it names any other file.
pub(crate) fn test_form(
    file: &Utf8Path,
    owns_dir: bool,
    path: Option<&str>,
) -> Result<TestForm, Vec<Option<String>>> {
    let stem = file.file_stem().unwrap_or_default();

    // A module named after a keyword, `use_.rs`, drops the underscore its file
    // name needs: its tests are `use_tests.rs`.
    let single = format!("{}_tests.rs", stem.strip_suffix('_').unwrap_or(stem));

    // A file owning its directory would load `tests.rs` beside itself, so it
    // names its test directory explicitly.
    let directory = owns_dir.then(|| format!("{stem}/tests.rs"));

    let single_path = file.parent().unwrap_or(Utf8Path::new("")).join(&single);
    let directory_path = file.with_extension("").join("tests.rs");

    match path {
        Some(path) if path == single => Ok(TestForm::Single {
            directory: directory_path,
        }),
        Some(path) if directory.as_deref() == Some(path) => Ok(TestForm::Directory {
            single: single_path,
        }),
        None if !owns_dir => Ok(TestForm::Directory {
            single: single_path,
        }),
        _ => Err(vec![Some(single), directory]),
    }
}

/// Whether `file` is a `mod.rs`.
pub(crate) fn is_mod_rs(file: &Utf8Path) -> bool {
    file.file_name() == Some("mod.rs")
}

/// Where a `mod.rs` file moves to: `foo/mod.rs` becomes `foo.rs`.
pub(crate) fn mod_rs_rename(file: &Utf8Path) -> Utf8PathBuf {
    file.parent()
        .map_or_else(|| file.to_owned(), |dir| dir.with_extension("rs"))
}

/// Whether `file` may hold `#[test]` functions: it is named `tests.rs` or
/// `*_tests.rs`.
pub(crate) fn holds_test_functions(file: &Utf8Path) -> bool {
    file.file_stem()
        .is_some_and(|stem| stem == "tests" || stem.ends_with("_tests"))
}

/// Whether a test file may declare a module loaded from `file`: another test
/// file, or the module's one `test_support.rs`.
pub(crate) fn may_declare_from_tests(file: &Utf8Path) -> bool {
    holds_test_functions(file) || file.file_name() == Some("test_support.rs")
}

/// Resolve `.` and `..` segments textually, keeping a leading `..` that has
/// nothing to cancel.
fn normalize(path: &Utf8Path) -> Utf8PathBuf {
    let mut out = Utf8PathBuf::new();
    for component in path.components() {
        match component {
            Utf8Component::CurDir => {}
            Utf8Component::ParentDir
                if matches!(out.components().next_back(), Some(Utf8Component::Normal(_))) =>
            {
                out.pop();
            }
            other => out.push(other.as_str()),
        }
    }
    out
}

#[cfg(test)]
#[path = "layout_tests.rs"]
mod tests;
