use super::*;

fn decl(name: &str, path: Option<&str>, inline: &[&str]) -> Declaration {
    Declaration {
        name: name.to_owned(),
        line: 1,
        path: path.map(ToOwned::to_owned),
        inline: inline.iter().map(|s| (*s).to_owned()).collect(),
    }
}

#[test]
fn a_file_not_owning_its_directory_looks_in_one_named_after_it() {
    assert_eq!(
        candidates(Utf8Path::new("src/foo.rs"), false, &decl("bar", None, &[])),
        [
            (Utf8PathBuf::from("src/foo/bar.rs"), false),
            (Utf8PathBuf::from("src/foo/bar/mod.rs"), true),
        ]
    );
}

#[test]
fn a_file_owning_its_directory_looks_beside_itself() {
    assert_eq!(
        candidates(Utf8Path::new("src/lib.rs"), true, &decl("bar", None, &[])),
        [
            (Utf8PathBuf::from("src/bar.rs"), false),
            (Utf8PathBuf::from("src/bar/mod.rs"), true),
        ]
    );
}

#[test]
fn a_top_level_path_is_relative_to_the_file_directory() {
    assert_eq!(
        candidates(
            Utf8Path::new("src/foo.rs"),
            false,
            &decl("tests", Some("foo_tests.rs"), &[])
        ),
        [(Utf8PathBuf::from("src/foo_tests.rs"), true)]
    );
}

#[test]
fn a_path_inside_an_inline_module_is_relative_to_its_directory() {
    assert_eq!(
        candidates(
            Utf8Path::new("src/foo.rs"),
            false,
            &decl("bar", Some("x.rs"), &["a"])
        ),
        [(Utf8PathBuf::from("src/foo/a/x.rs"), true)]
    );
    assert_eq!(
        candidates(
            Utf8Path::new("src/foo.rs"),
            false,
            &decl("bar", None, &["a"])
        ),
        [
            (Utf8PathBuf::from("src/foo/a/bar.rs"), false),
            (Utf8PathBuf::from("src/foo/a/bar/mod.rs"), true),
        ]
    );
}

#[test]
fn a_path_with_parent_segments_is_resolved() {
    assert_eq!(
        candidates(
            Utf8Path::new("src/foo/tests.rs"),
            true,
            &decl("other", Some("../other_tests.rs"), &[])
        ),
        [(Utf8PathBuf::from("src/other_tests.rs"), true)]
    );
}

#[test]
fn each_test_form_names_the_other() {
    assert_eq!(
        test_form(Utf8Path::new("src/foo.rs"), false, Some("foo_tests.rs")),
        Ok(TestForm::Single {
            directory: Utf8PathBuf::from("src/foo/tests.rs")
        })
    );
    assert_eq!(
        test_form(Utf8Path::new("src/lib.rs"), true, Some("lib/tests.rs")),
        Ok(TestForm::Directory {
            single: Utf8PathBuf::from("src/lib_tests.rs")
        })
    );
}

/// `use_.rs` is named so because `use` is a keyword; its tests drop the
/// underscore, wherever they live.
#[test]
fn a_keyword_module_drops_its_underscore_in_the_single_form() {
    assert_eq!(
        test_form(Utf8Path::new("src/use_.rs"), false, Some("use_tests.rs")),
        Ok(TestForm::Single {
            directory: Utf8PathBuf::from("src/use_/tests.rs")
        })
    );
    assert_eq!(
        test_form(Utf8Path::new("src/use_.rs"), false, None),
        Ok(TestForm::Directory {
            single: Utf8PathBuf::from("src/use_tests.rs")
        })
    );
}

#[test]
fn a_mod_rs_moves_up_a_directory() {
    assert_eq!(
        mod_rs_rename(Utf8Path::new("src/config/mod.rs")),
        Utf8PathBuf::from("src/config.rs")
    );
}
