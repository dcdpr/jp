use std::sync::Arc;

use camino_tempfile::{Utf8TempDir, tempdir};
use jp_config::AppConfig;
use jp_conversation::{Conversation, ConversationId};
use jp_printer::{OutputFormat, Printer, SharedBuffer};
use jp_storage::backend::FsStorageBackend;
use jp_workspace::Workspace;
use tokio::runtime::Runtime;

use super::*;
use crate::{Globals, cmd::conversation_id::PositionalIds, ctx::Ctx};

fn make_id(secs: u64) -> ConversationId {
    ConversationId::try_from(
        chrono::DateTime::<chrono::Utc>::UNIX_EPOCH + std::time::Duration::from_secs(secs),
    )
    .unwrap()
}

/// Set up a workspace with a conversation persisted to disk.
///
/// Returns the temp dir handle to keep it alive for the test's duration.
fn setup(id: ConversationId) -> (Ctx, SharedBuffer, Utf8TempDir) {
    let tmp = tempdir().unwrap();
    let storage_path = tmp.path().join(".jp");
    let fs = Arc::new(FsStorageBackend::new(&storage_path).unwrap());
    fs.write_test_conversation(&id, &Conversation::default());

    let config = AppConfig::new_test();
    let mut workspace = Workspace::new(tmp.path()).with_backend(fs.clone());
    workspace.load_conversation_index();

    let (printer, out, _err) = Printer::memory(OutputFormat::Text);
    let ctx = Ctx::new(
        workspace,
        Some(fs),
        Runtime::new().unwrap(),
        Globals::default(),
        config,
        None,
        printer,
    );

    (ctx, out, tmp)
}

#[test]
fn prints_conversation_directory() {
    let id = make_id(1000);
    let (mut ctx, out, _tmp) = setup(id);
    let handle = ctx.workspace.acquire_conversation(&id).unwrap();

    let path_cmd = Path {
        target: PositionalIds::from_targets(vec![]),
        events: false,
        metadata: false,
        base_config: false,
    };

    path_cmd.run(&mut ctx, vec![handle]).unwrap();
    ctx.printer.flush();
    let output = out.lock().clone();
    let path = output.trim();

    assert!(path.ends_with(&id.to_dirname(None)), "got: {path}");
    assert!(!path.contains("events.json"));
}

#[test]
fn prints_events_path() {
    let id = make_id(2000);
    let (mut ctx, out, _tmp) = setup(id);
    let handle = ctx.workspace.acquire_conversation(&id).unwrap();

    let path_cmd = Path {
        target: PositionalIds::from_targets(vec![]),
        events: true,
        metadata: false,
        base_config: false,
    };

    path_cmd.run(&mut ctx, vec![handle]).unwrap();
    ctx.printer.flush();
    let output = out.lock().clone();

    assert!(output.trim().ends_with("events.json"), "got: {output}");
}

#[test]
fn prints_metadata_path() {
    let id = make_id(3000);
    let (mut ctx, out, _tmp) = setup(id);
    let handle = ctx.workspace.acquire_conversation(&id).unwrap();

    let path_cmd = Path {
        target: PositionalIds::from_targets(vec![]),
        events: false,
        metadata: true,
        base_config: false,
    };

    path_cmd.run(&mut ctx, vec![handle]).unwrap();
    ctx.printer.flush();
    let output = out.lock().clone();

    assert!(output.trim().ends_with("metadata.json"), "got: {output}");
}

#[test]
fn prints_base_config_path() {
    let id = make_id(4000);
    let (mut ctx, out, _tmp) = setup(id);
    let handle = ctx.workspace.acquire_conversation(&id).unwrap();

    let path_cmd = Path {
        target: PositionalIds::from_targets(vec![]),
        events: false,
        metadata: false,
        base_config: true,
    };

    path_cmd.run(&mut ctx, vec![handle]).unwrap();
    ctx.printer.flush();
    let output = out.lock().clone();

    assert!(output.trim().ends_with("base_config.json"), "got: {output}");
}

#[test]
fn prints_multiple_file_paths() {
    let id = make_id(5000);
    let (mut ctx, out, _tmp) = setup(id);
    let handle = ctx.workspace.acquire_conversation(&id).unwrap();

    let path_cmd = Path {
        target: PositionalIds::from_targets(vec![]),
        events: true,
        metadata: true,
        base_config: false,
    };

    path_cmd.run(&mut ctx, vec![handle]).unwrap();
    ctx.printer.flush();
    let output = out.lock().clone();
    let lines: Vec<&str> = output.trim().lines().collect();

    assert_eq!(lines.len(), 2);
    assert!(lines[0].ends_with("events.json"), "got: {}", lines[0]);
    assert!(lines[1].ends_with("metadata.json"), "got: {}", lines[1]);
}
