use camino::Utf8PathBuf;
use camino_tempfile::tempdir;
use jp_process::MockProcessRunner;

use super::*;

mod suppress_matcher {
    use super::*;

    #[test]
    fn rejects_an_unparseable_pattern_naming_it() {
        // A disclosure control that drops the rule it could not read hands over
        // the very paths it was configured to hold back.
        let dir = tempdir().unwrap();
        let err =
            super::super::suppress_matcher(dir.path(), &["{unclosed".to_owned()]).unwrap_err();

        assert!(err.contains("{unclosed"), "pattern not named: {err}");
        assert!(
            err.starts_with("Invalid `suppress` pattern"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn one_bad_pattern_rejects_the_whole_list() {
        let dir = tempdir().unwrap();
        let patterns = [".git/".to_owned(), "{unclosed".to_owned()];

        assert!(super::super::suppress_matcher(dir.path(), &patterns).is_err());
    }

    #[test]
    fn accepts_an_empty_list() {
        let dir = tempdir().unwrap();
        let matcher = super::super::suppress_matcher(dir.path(), &[]).unwrap();

        assert!(!is_suppressed(&matcher, &[Utf8Path::new("anything")]));
    }
}

#[test]
fn test_is_file_dirty_modified() {
    let dir = tempdir().unwrap();
    let file = Utf8PathBuf::from("test.rs");

    // Second column 'M' indicates modified
    let runner = MockProcessRunner::success(" M test.rs\n");

    let result = is_file_dirty_impl(dir.path(), &file, &runner).unwrap();

    assert!(result);
}

#[test]
fn test_is_file_dirty_not_modified() {
    let dir = tempdir().unwrap();
    let file = Utf8PathBuf::from("test.rs");

    // No output means no changes
    let runner = MockProcessRunner::success("");

    let result = is_file_dirty_impl(dir.path(), &file, &runner).unwrap();

    assert!(!result);
}

#[test]
fn test_is_file_dirty_not_a_git_repo() {
    let dir = tempdir().unwrap();
    let file = Utf8PathBuf::from("test.rs");

    let runner = MockProcessRunner::error("fatal: not a git repository");

    let result = is_file_dirty_impl(dir.path(), &file, &runner).unwrap();

    // Should return false when not in a git repo
    assert!(!result);
}

/// The boundary itself is `jp_tool::resolve_path`'s and is tested there.
/// These pin what this layer adds: the input limits, and the words the
/// assistant reads.
mod resolve_workspace_path {
    use super::*;

    #[test]
    fn rejects_empty_path() {
        let dir = tempdir().unwrap();
        for path in ["", ".", "a/.."] {
            assert_eq!(
                resolve_workspace_path(dir.path(), path, None),
                Err("Path must not be empty.".to_owned()),
                "path: {path:?}"
            );
        }
    }

    #[test]
    fn rejects_oversized_component() {
        let dir = tempdir().unwrap();
        let long = "a".repeat(101);
        assert_eq!(
            resolve_workspace_path(dir.path(), &long, None),
            Err("Individual path components must be less than 100 characters long.".to_owned())
        );
    }

    #[test]
    fn rejects_too_many_components() {
        let dir = tempdir().unwrap();
        let deep = vec!["a"; 21].join("/");
        assert_eq!(
            resolve_workspace_path(dir.path(), &deep, None),
            Err("Path must be less than 20 components long.".to_owned())
        );
    }

    /// Components a `..` cancels do not count toward the limit.
    #[test]
    fn counts_components_after_normalizing() {
        let dir = tempdir().unwrap();
        let path = format!("{}/{}", vec!["a"; 19].join("/"), "b/../c");
        assert!(resolve_workspace_path(dir.path(), &path, None).is_ok());
    }

    #[test]
    fn names_what_is_wrong_with_a_refused_path() {
        let dir = tempdir().unwrap();
        assert_eq!(
            resolve_workspace_path(dir.path(), "/etc/passwd", None),
            Err("Path must be relative.".to_owned())
        );
        assert_eq!(
            resolve_workspace_path(dir.path(), "foo/../../etc/passwd", None),
            Err("Path must not escape the workspace root.".to_owned())
        );
    }

    #[cfg(unix)]
    #[test]
    fn names_the_dangling_link_by_its_workspace_path() {
        let workspace = tempdir().unwrap();
        std::os::unix::fs::symlink(
            workspace.path().join("missing"),
            workspace.path().join("dangling"),
        )
        .unwrap();

        assert_eq!(
            resolve_workspace_path(workspace.path(), "dangling/child.rs", None),
            Err("Path 'dangling' is a symlink with a missing or non-workspace target.".to_owned())
        );
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_symlink_out_of_the_workspace() {
        let outside = tempdir().unwrap();
        let workspace = tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), workspace.path().join("linkdir")).unwrap();

        assert_eq!(
            resolve_workspace_entry(workspace.path(), "linkdir/new.rs", None),
            Err("Path escapes the workspace root.".to_owned())
        );
    }
}

mod authorize {
    use jp_tool::FsRule;

    use super::*;

    #[test]
    fn a_denial_lists_the_granting_paths_and_names_the_user() {
        let policy = AccessPolicy {
            fs: vec![
                FsRule::new("").with_read(true),
                FsRule::new("build").with_write(true),
            ],
            ..AccessPolicy::default()
        };

        assert_eq!(
            authorize_entry(
                Some(&policy),
                Capability::Delete,
                Utf8Path::new("src"),
                true
            ),
            Err(
                "Access denied: cannot delete 'src'. Paths granting delete: [build]. If required, \
                 ask the user for explicit access."
                    .to_owned()
            )
        );
        assert_eq!(
            authorize_entry(None, Capability::Delete, Utf8Path::new("src"), true),
            Ok(())
        );
    }
}
