//! Tests for [`build_editor_backend`]: the seam from `EditorConfig` to a
//! runnable [`EditorBackend`].
//!
//! These exercise the full path a real edit takes — `editor.cmd` resolved by
//! `command()`, wrapped in a `TerminalEditorBackend` — with a fake `sh`-based
//! editor standing in for a real one, so they catch argument-forwarding
//! regressions across the crate boundary.
#![cfg(unix)]

use camino_tempfile::NamedUtf8TempFile;
use jp_config::editor::{EditorConfig, InlineEditorConfig};
use jp_editor::{EditOutcome, EditRequest};

use super::build_editor_backend;

fn editor_config(cmd: &str) -> EditorConfig {
    EditorConfig {
        cmd: Some(cmd.to_owned()),
        envs: vec![],
        inline: InlineEditorConfig::default(),
    }
}

/// `edit_text` through `editor.cmd` reaches the temp file: the fake editor
/// writes known content into the appended path, and the read-back proves the
/// path was forwarded.
/// The regression this guards against left `$EDITOR` pointed at the wrong
/// argument, so the seed came back unchanged.
#[test]
fn cmd_edit_text_round_trips() {
    let backend = build_editor_backend(&editor_config("printf 'EDITED' >")).unwrap();

    let (outcome, content) = backend.edit_text("seed").unwrap();

    assert_eq!(outcome, EditOutcome::Saved);
    assert_eq!(content, "EDITED");
}

/// `edit_file` through `editor.cmd` opens the caller's path.
#[test]
fn cmd_edit_file_writes_caller_path() {
    let tmp = NamedUtf8TempFile::new().unwrap();
    std::fs::write(tmp.path(), "before").unwrap();

    let backend = build_editor_backend(&editor_config("printf 'AFTER' >")).unwrap();

    let path = tmp.path().to_owned();
    let outcome = backend
        .edit_file(EditRequest {
            paths: std::slice::from_ref(&path),
            cwd: None,
        })
        .unwrap();

    assert_eq!(outcome, EditOutcome::Saved);
    assert_eq!(std::fs::read_to_string(tmp.path()).unwrap(), "AFTER");
}
