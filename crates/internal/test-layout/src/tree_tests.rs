use std::{collections::BTreeMap, io};

use indoc::indoc;
use pretty_assertions::assert_eq;

use super::*;

/// Source files held in memory, keyed by path.
struct Files(BTreeMap<Utf8PathBuf, String>);

impl Sources for Files {
    fn read(&self, path: &Utf8Path) -> io::Result<String> {
        self.0
            .get(path)
            .cloned()
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }

    fn exists(&self, path: &Utf8Path) -> bool {
        self.0.contains_key(path)
    }
}

/// Check a crate rooted at `src/lib.rs`.
fn check_files(files: &[(&str, &str)]) -> Vec<Violation> {
    let files = Files(
        files
            .iter()
            .map(|(path, source)| (Utf8PathBuf::from(*path), (*source).to_owned()))
            .collect(),
    );
    check([Utf8PathBuf::from("src/lib.rs")], &files)
}

fn at(file: &str, line: usize, breach: Breach) -> Violation {
    Violation {
        file: Utf8PathBuf::from(file),
        line,
        breach,
    }
}

#[test]
fn a_module_with_one_test_module_passes() {
    let violations = check_files(&[
        ("src/lib.rs", indoc! {r#"
            pub fn run() {}

            #[cfg(test)]
            #[path = "lib_tests.rs"]
            mod tests;
        "#}),
        ("src/lib_tests.rs", "#[test]\nfn runs() {}\n"),
    ]);

    assert_eq!(violations, []);
}

#[test]
fn a_module_with_several_test_modules_passes() {
    let violations = check_files(&[
        ("src/lib.rs", "mod foo;\n"),
        ("src/foo.rs", indoc! {"
            pub fn run() {}

            #[cfg(test)]
            mod tests;
        "}),
        ("src/foo/tests.rs", indoc! {r#"
            #[path = "other_tests.rs"]
            mod other_tests;

            #[path = "test_support.rs"]
            pub(crate) mod test_support;

            #[test]
            fn runs() {}
        "#}),
        (
            "src/foo/other_tests.rs",
            "#[tokio::test]\nasync fn waits() {}\n",
        ),
        ("src/foo/test_support.rs", "pub(crate) fn fixture() {}\n"),
    ]);

    assert_eq!(violations, []);
}

/// `use_.rs` is named so because `use` is a keyword, and its test file drops
/// the underscore.
#[test]
fn a_keyword_module_keeps_its_tests_in_its_own_name() {
    let violations = check_files(&[
        ("src/lib.rs", "mod use_;\n"),
        (
            "src/use_.rs",
            "#[cfg(test)]\n#[path = \"use_tests.rs\"]\nmod tests;\n",
        ),
        ("src/use_tests.rs", "#[test]\nfn runs() {}\n"),
    ]);

    assert_eq!(violations, []);
}

/// `all(test, unix)` requires `test` as much as `test` alone does, so it may
/// sit on the trailing declaration.
#[test]
fn a_platform_gated_test_module_passes() {
    let violations = check_files(&[
        ("src/lib.rs", indoc! {r#"
            #[cfg(all(test, unix))]
            #[path = "lib_tests.rs"]
            mod tests;
        "#}),
        ("src/lib_tests.rs", "#[test]\nfn runs() {}\n"),
    ]);

    assert_eq!(violations, []);
}

#[test]
fn every_kind_of_test_only_code_is_reported() {
    let violations = check_files(&[("src/lib.rs", indoc! {"
        #[cfg(test)]
        use std::fmt;

        pub struct Ctx {
            #[cfg(test)]
            now: u64,
        }

        impl Ctx {
            pub fn new() -> Self {
                Self {
                    #[cfg(test)]
                    now: 0,
                }
            }

            #[cfg(test)]
            fn fixture() -> Self {
                Self::new()
            }
        }

        pub enum Path {
            File,
            #[cfg(all(test, unix))]
            Memory,
        }

        pub fn open(path: Path) -> u8 {
            #[cfg(test)]
            let stubbed = 1;
            match path {
                Path::File => 0,
                #[cfg(test)]
                Path::Memory => 1,
            }
        }
    "})]);

    assert_eq!(violations, [
        at("src/lib.rs", 1, Breach::TestOnly { what: "import" }),
        at("src/lib.rs", 5, Breach::TestOnly { what: "field" }),
        at("src/lib.rs", 12, Breach::TestOnly {
            what: "field initializer"
        }),
        at("src/lib.rs", 17, Breach::TestOnly { what: "method" }),
        at("src/lib.rs", 25, Breach::TestOnly { what: "variant" }),
        at("src/lib.rs", 30, Breach::TestOnly { what: "statement" }),
        at("src/lib.rs", 34, Breach::TestOnly { what: "match arm" }),
    ]);
}

/// `not(test)` is reported, while a feature-gated `any(test, …)` module, a
/// `cfg_attr(test, …)`, and a `#[cfg(test)]` inside a string are not.
#[test]
fn only_code_that_differs_under_test_is_reported() {
    let violations = check_files(&[
        ("src/lib.rs", indoc! {r##"
            #[cfg(not(test))]
            pub fn now() -> u64 {
                0
            }

            #[cfg(any(test, feature = "mock"))]
            pub mod mock;

            #[cfg_attr(test, derive(Debug))]
            pub struct Shown;

            const FIXTURE: &str = "#[cfg(test)]\nmod tests { }";
        "##}),
        ("src/mock.rs", "pub struct Mock;\n"),
    ]);

    assert_eq!(violations, [at("src/lib.rs", 1, Breach::NotTest)]);
}

/// The production files a crate declares are walked too, not only its root.
#[test]
fn breaches_in_child_modules_are_reported() {
    let violations = check_files(&[
        ("src/lib.rs", "mod mock;\n"),
        (
            "src/mock.rs",
            "mod inner {\n    #[cfg(test)]\n    fn helper() {}\n}\n",
        ),
    ]);

    assert_eq!(violations, [at("src/mock.rs", 2, Breach::TestOnly {
        what: "function"
    })]);
}

#[test]
fn an_inline_test_module_is_reported_once() {
    let violations = check_files(&[("src/lib.rs", indoc! {"
        pub fn run() {}

        #[cfg(test)]
        mod tests {
            #[test]
            fn runs() {}
        }
    "})]);

    assert_eq!(violations, [at("src/lib.rs", 4, Breach::InlineTestModule)]);
}

#[test]
fn test_functions_in_a_production_file_are_reported_together() {
    let violations = check_files(&[("src/lib.rs", indoc! {"
        pub fn run() {}

        #[test]
        fn runs() {}

        #[test]
        fn walks() {}
    "})]);

    assert_eq!(violations, [at("src/lib.rs", 3, Breach::TestFunctions {
        count: 2
    })]);
}

#[test]
fn only_one_test_module_declaration_closes_a_file() {
    let violations = check_files(&[
        ("src/lib.rs", indoc! {r#"
            #[cfg(test)]
            #[path = "lib/test_support.rs"]
            pub(crate) mod test_support;

            #[cfg(test)]
            #[path = "lib_tests.rs"]
            mod tests;

            pub fn run() {}
        "#}),
        ("src/lib/test_support.rs", "pub fn fixture() {}\n"),
        ("src/lib_tests.rs", "#[test]\nfn runs() {}\n"),
    ]);

    assert_eq!(violations, [
        at("src/lib.rs", 3, Breach::ExtraTestModule {
            name: "test_support".to_owned()
        }),
        at("src/lib.rs", 7, Breach::DeclarationNotLast),
    ]);
}

/// A crate root owns its directory, so a bare `mod tests;` would load
/// `src/tests.rs` beside it: the multi-module form names `lib/tests.rs`.
#[test]
fn a_crate_root_names_its_test_directory() {
    let violations = check_files(&[
        ("src/lib.rs", "#[cfg(test)]\nmod tests;\n"),
        ("src/tests.rs", "#[test]\nfn runs() {}\n"),
    ]);

    assert_eq!(violations, [at("src/lib.rs", 2, Breach::TestModulePath {
        found: None,
        expected: vec![
            Some("lib_tests.rs".to_owned()),
            Some("lib/tests.rs".to_owned())
        ],
    })]);
}

/// A file loaded through a `#[path]` owns its directory, as a crate root does.
#[test]
fn a_path_loaded_file_names_its_test_directory() {
    let violations = check_files(&[
        ("src/lib.rs", "#[path = \"imp/thing.rs\"]\nmod thing;\n"),
        ("src/imp/thing.rs", "#[cfg(test)]\nmod tests;\n"),
        ("src/imp/tests.rs", "#[test]\nfn runs() {}\n"),
    ]);

    assert_eq!(violations, [at(
        "src/imp/thing.rs",
        2,
        Breach::TestModulePath {
            found: None,
            expected: vec![
                Some("thing_tests.rs".to_owned()),
                Some("thing/tests.rs".to_owned())
            ],
        }
    )]);
}

#[test]
fn a_module_loads_its_test_directory_without_a_path() {
    let violations = check_files(&[
        ("src/lib.rs", "mod foo;\n"),
        (
            "src/foo.rs",
            "#[cfg(test)]\n#[path = \"foo/tests.rs\"]\nmod tests;\n",
        ),
        (
            "src/foo/tests.rs",
            "#[path = \"a_tests.rs\"]\nmod a_tests;\n",
        ),
        ("src/foo/a_tests.rs", ""),
    ]);

    assert_eq!(violations, [at("src/foo.rs", 3, Breach::TestModulePath {
        found: Some("foo/tests.rs".to_owned()),
        expected: vec![Some("foo_tests.rs".to_owned()), None],
    })]);
}

#[test]
fn a_mod_rs_file_is_reported() {
    let violations = check_files(&[
        ("src/lib.rs", "mod foo;\n"),
        ("src/foo/mod.rs", "pub fn run() {}\n"),
    ]);

    assert_eq!(violations, [at("src/foo/mod.rs", 1, Breach::ModRs {
        rename: Utf8PathBuf::from("src/foo.rs")
    })]);
}

#[test]
fn a_test_file_names_its_modules_as_siblings() {
    let violations = check_files(&[
        ("src/lib.rs", "mod foo;\n"),
        ("src/foo.rs", "#[cfg(test)]\nmod tests;\n"),
        ("src/foo/tests.rs", indoc! {r#"
            mod helpers;

            #[path = "../other_tests.rs"]
            mod other_tests;
        "#}),
        ("src/foo/tests/helpers.rs", ""),
        ("src/other_tests.rs", ""),
    ]);

    assert_eq!(violations, [
        at("src/foo/tests.rs", 1, Breach::UnpathedTestModule {
            name: "helpers".to_owned()
        }),
        at("src/foo/tests.rs", 4, Breach::NotSibling {
            path: "../other_tests.rs".to_owned()
        }),
    ]);
}

/// Helpers and test doubles share one name, so every tool that tells test code
/// from production by file name finds them.
#[test]
fn a_test_file_declares_only_test_files_and_test_support() {
    let violations = check_files(&[
        ("src/lib.rs", "mod foo;\n"),
        ("src/foo.rs", "#[cfg(test)]\nmod tests;\n"),
        ("src/foo/tests.rs", indoc! {r#"
            #[path = "mock.rs"]
            pub(crate) mod mock;

            #[path = "test_support.rs"]
            pub(crate) mod test_support;

            #[path = "other_tests.rs"]
            mod other_tests;
        "#}),
        ("src/foo/mock.rs", ""),
        ("src/foo/test_support.rs", ""),
        ("src/foo/other_tests.rs", ""),
    ]);

    assert_eq!(violations, [at(
        "src/foo/tests.rs",
        2,
        Breach::SupportModuleName {
            path: "mock.rs".to_owned()
        }
    )]);
}

#[test]
fn a_single_test_file_declares_no_further_modules() {
    let violations = check_files(&[
        (
            "src/lib.rs",
            "#[cfg(test)]\n#[path = \"lib_tests.rs\"]\nmod tests;\n",
        ),
        (
            "src/lib_tests.rs",
            "#[path = \"other_tests.rs\"]\nmod other_tests;\n",
        ),
        ("src/other_tests.rs", ""),
    ]);

    assert_eq!(violations, [at(
        "src/lib_tests.rs",
        2,
        Breach::SingleFileDeclaresModules {
            move_to: Utf8PathBuf::from("src/lib/tests.rs")
        }
    )]);
}

#[test]
fn a_test_directory_holds_more_than_one_module() {
    let violations = check_files(&[
        ("src/lib.rs", "mod foo;\n"),
        ("src/foo.rs", "#[cfg(test)]\nmod tests;\n"),
        ("src/foo/tests.rs", "#[test]\nfn runs() {}\n"),
    ]);

    assert_eq!(violations, [at(
        "src/foo/tests.rs",
        1,
        Breach::DirectoryWithOneModule {
            rename: Utf8PathBuf::from("src/foo_tests.rs")
        }
    )]);
}

#[test]
fn test_functions_stay_out_of_test_support() {
    let violations = check_files(&[
        ("src/lib.rs", "mod foo;\n"),
        ("src/foo.rs", "#[cfg(test)]\nmod tests;\n"),
        (
            "src/foo/tests.rs",
            "#[path = \"test_support.rs\"]\npub(crate) mod test_support;\n",
        ),
        ("src/foo/test_support.rs", "#[test]\nfn runs() {}\n"),
    ]);

    assert_eq!(violations, [at(
        "src/foo/test_support.rs",
        1,
        Breach::TestFunctions { count: 1 }
    )]);
}

#[test]
fn a_module_without_a_file_is_reported() {
    let violations = check_files(&[("src/lib.rs", "mod gone;\n")]);

    assert_eq!(violations, [at("src/lib.rs", 1, Breach::MissingModule {
        name: "gone".to_owned()
    })]);
}
