//! Tests for [`build_editor_backend`]: the seam from `EditorConfig` to a
//! runnable [`EditorBackend`].
//!
//! These exercise the full path a real edit takes — `editor.cmd` resolved by
//! `command()`, wrapped in a `TerminalEditorBackend` — with a fake `sh`-based
//! editor standing in for a real one, so they catch argument-forwarding
//! regressions across the crate boundary.
#![cfg(unix)]

use camino_tempfile::NamedUtf8TempFile;
use jp_config::{
    conversation::tool::{CommandConfig, CommandConfigOrString},
    editor::{EditorConfig, InlineEditorConfig},
};
use jp_editor::{EditOutcome, EditRequest};

use super::build_editor_backend;

fn editor_config(cmd: CommandConfigOrString) -> EditorConfig {
    EditorConfig {
        cmd: Some(cmd),
        envs: vec![],
        inline: InlineEditorConfig::default(),
    }
}

/// A string `cmd` (default `shell = false`): a fake editor that overwrites its
/// first argument (`$1`) simulates an edit-and-save.
fn string_cmd(script: &str) -> CommandConfigOrString {
    CommandConfigOrString::String(format!("sh -c '{script}' jp-fake"))
}

/// A `shell = true` `cmd`: the appended path is forwarded via `"$@"`.
fn shell_cmd(program: &str) -> CommandConfigOrString {
    CommandConfigOrString::Config(CommandConfig {
        program: program.to_owned(),
        args: vec![],
        shell: true,
    })
}

/// `edit_text` through a `shell = false` `editor.cmd` reaches the temp file:
/// the fake editor writes known content into the appended path (`$1`), and the
/// read-back proves the path was forwarded as a direct argument.
#[test]
fn cmd_edit_text_round_trips() {
    let backend =
        build_editor_backend(&editor_config(string_cmd(r#"printf EDITED > "$1""#))).unwrap();

    let (outcome, content) = backend.edit_text("seed").unwrap();

    assert_eq!(outcome, EditOutcome::Saved);
    assert_eq!(content, "EDITED");
}

/// `edit_file` through a `shell = false` `editor.cmd` opens the caller's path.
#[test]
fn cmd_edit_file_writes_caller_path() {
    let tmp = NamedUtf8TempFile::new().unwrap();
    std::fs::write(tmp.path(), "before").unwrap();

    let backend =
        build_editor_backend(&editor_config(string_cmd(r#"printf AFTER > "$1""#))).unwrap();

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

/// `edit_text` through a `shell = true` `editor.cmd` forwards the temp file via
/// `"$@"`, so a redirect-based fake editor still reaches it.
#[test]
fn cmd_shell_edit_text_round_trips() {
    let backend = build_editor_backend(&editor_config(shell_cmd("printf SHELL-EDIT >"))).unwrap();

    let (outcome, content) = backend.edit_text("seed").unwrap();

    assert_eq!(outcome, EditOutcome::Saved);
    assert_eq!(content, "SHELL-EDIT");
}
