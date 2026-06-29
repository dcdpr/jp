//! Integration tests for [`TerminalEditorBackend`].
//!
//! A fake "editor" is spawned via `sh -c` so the tests can assert that the
//! edited path is passed as an argument (the arg-preserving contract) and that
//! the editor's exit status maps to the right [`EditOutcome`], without a real
//! editor or a tty.
#![cfg(unix)]

use camino_tempfile::NamedUtf8TempFile;
use jp_editor::{EditOutcome, EditRequest, EditorBackend, TerminalEditorBackend};

/// `sh -c <script> sh <path>` exposes the appended path as `$1`, so a script
/// that overwrites `$1` simulates an edit-and-save and proves the path reached
/// the command as an argument.
#[test]
fn edit_text_passes_path_and_saves_edited_content() {
    let backend =
        TerminalEditorBackend::new(duct::cmd("sh", ["-c", r#"printf 'edited\n' > "$1""#, "sh"]));

    let (outcome, content) = backend.edit_text("seed").unwrap();

    assert_eq!(outcome, EditOutcome::Saved);
    assert_eq!(content, "edited\n");
}

/// A non-zero editor exit maps to `Cancelled`, matching git's commit-message
/// convention.
#[test]
fn edit_text_nonzero_exit_maps_to_cancelled() {
    let backend = TerminalEditorBackend::new(duct::cmd("sh", ["-c", "exit 1", "sh"]));

    let (outcome, _) = backend.edit_text("seed").unwrap();

    assert_eq!(outcome, EditOutcome::Cancelled);
}

/// `edit_file` opens the editor on the caller's path and reports the outcome;
/// the caller reads the file back itself.
#[test]
fn edit_file_runs_on_caller_path() {
    let tmp = NamedUtf8TempFile::new().unwrap();
    std::fs::write(tmp.path(), "before").unwrap();

    let backend =
        TerminalEditorBackend::new(duct::cmd("sh", ["-c", r#"printf 'after\n' > "$1""#, "sh"]));

    let path = tmp.path().to_owned();
    let outcome = backend
        .edit_file(EditRequest {
            paths: std::slice::from_ref(&path),
            cwd: None,
        })
        .unwrap();

    assert_eq!(outcome, EditOutcome::Saved);
    assert_eq!(std::fs::read_to_string(tmp.path()).unwrap(), "after\n");
}
