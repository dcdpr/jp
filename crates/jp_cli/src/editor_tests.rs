//! Tests for [`build_editor_backend`]: the seam from `EditorConfig` to a
//! runnable [`EditorBackend`].
//!
//! These exercise the full path a real edit takes — `editor.cmd` resolved by
//! `command()`, wrapped in a `TerminalEditorBackend` — with a fake `sh`-based
//! editor standing in for a real one, so they catch argument-forwarding
//! regressions across the crate boundary.
#![cfg(unix)]

use std::{sync::Arc, time::Duration};

use camino_tempfile::NamedUtf8TempFile;
use jp_config::{
    conversation::tool::{CommandConfig, CommandConfigOrString},
    editor::{EditorConfig, InlineEditorConfig},
};
use jp_editor::{EditOutcome, EditRequest, EditorBackend};
use jp_printer::{OutputFormat, Printer, RegionStyle, SharedBuffer, TerminalCapability};

use super::{EditorResult, SuspendingEditor, build_editor_backend};

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

/// An editor that snapshots the chrome buffer at the moment it is handed the
/// terminal.
struct ObservingEditor {
    /// The printer's chrome (stderr) buffer.
    chrome: SharedBuffer,

    /// What the chrome buffer held when the editor ran.
    seen: SharedBuffer,
}

impl ObservingEditor {
    /// Record the chrome buffer as it stands right now.
    fn observe(&self) {
        let snapshot = self.chrome.lock().clone();
        self.seen.lock().push_str(&snapshot);
    }
}

impl EditorBackend for ObservingEditor {
    fn edit_text(&self, _content: &str) -> EditorResult<(EditOutcome, String)> {
        self.observe();
        Ok((EditOutcome::Saved, "edited".to_owned()))
    }

    fn edit_file(&self, _req: EditRequest<'_>) -> EditorResult<EditOutcome> {
        self.observe();
        Ok(EditOutcome::Saved)
    }
}

/// The editor is a child process painting the terminal directly, so the status
/// region has to be gone *before* it starts — not merely enqueued for erasure.
#[test]
fn an_edit_erases_the_status_region_before_the_editor_runs() {
    let (printer, _out, err) = Printer::memory(OutputFormat::TextPretty);
    let printer = printer.with_terminal(TerminalCapability::interactive(Some(80)));

    let _region = printer.status_region(RegionStyle::new(
        Duration::ZERO,
        Duration::from_millis(10),
        |_, _| "waiting".to_owned(),
    ));
    printer.flush();
    assert_eq!(*err.lock(), "\r\x1b[Kwaiting");
    err.lock().clear();

    let observer = Arc::new(ObservingEditor {
        chrome: Arc::clone(&err),
        seen: SharedBuffer::default(),
    });
    let backend = SuspendingEditor {
        inner: Arc::clone(&observer) as Arc<dyn EditorBackend>,
        printer: printer.clone(),
    };

    let (outcome, content) = backend.edit_text("seed").unwrap();
    assert_eq!(outcome, EditOutcome::Saved);
    assert_eq!(content, "edited");

    // An empty snapshot would mean the editor never ran; anything with
    // `waiting` in it would mean the row was still on screen when it did.
    assert_eq!(
        *observer.seen.lock(),
        "\r\x1b[K",
        "the row must already be erased when the editor takes over"
    );

    // The region comes back once the edit is done.
    printer.flush();
    assert_eq!(*err.lock(), "\r\x1b[K\r\x1b[Kwaiting");
}

/// `edit_text` through a `shell = false` `editor.cmd` reaches the temp file:
/// the fake editor writes known content into the appended path (`$1`), and the
/// read-back proves the path was forwarded as a direct argument.
#[test]
fn cmd_edit_text_round_trips() {
    let backend = build_editor_backend(
        &editor_config(string_cmd(r#"printf EDITED > "$1""#)),
        &Printer::sink(),
    )
    .unwrap();

    let (outcome, content) = backend.edit_text("seed").unwrap();

    assert_eq!(outcome, EditOutcome::Saved);
    assert_eq!(content, "EDITED");
}

/// `edit_file` through a `shell = false` `editor.cmd` opens the caller's path.
#[test]
fn cmd_edit_file_writes_caller_path() {
    let tmp = NamedUtf8TempFile::new().unwrap();
    std::fs::write(tmp.path(), "before").unwrap();

    let backend = build_editor_backend(
        &editor_config(string_cmd(r#"printf AFTER > "$1""#)),
        &Printer::sink(),
    )
    .unwrap();

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
    let backend = build_editor_backend(
        &editor_config(shell_cmd("printf SHELL-EDIT >")),
        &Printer::sink(),
    )
    .unwrap();

    let (outcome, content) = backend.edit_text("seed").unwrap();

    assert_eq!(outcome, EditOutcome::Saved);
    assert_eq!(content, "SHELL-EDIT");
}
