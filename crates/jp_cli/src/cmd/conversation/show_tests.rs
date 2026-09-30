use camino_tempfile::tempdir;
use chrono::{DateTime, Utc};
use jp_config::AppConfig;
use jp_conversation::{Conversation, ConversationId};
use jp_printer::{OutputFormat, Printer, SharedBuffer};
use jp_workspace::{ConversationHandle, Workspace};
use strip_ansi_escapes::strip_str;

use super::*;
use crate::Globals;

/// A context holding one conversation titled `title`, printing in `format` to
/// the returned buffer, and the handle `show` is given.
fn setup(title: &str, format: OutputFormat) -> (Ctx, SharedBuffer, ConversationHandle) {
    let tmp = tempdir().unwrap();
    let workspace = Workspace::in_memory(tmp.path());
    let (printer, out, _err) = Printer::memory(format);
    let mut ctx = Ctx::new(
        crate::bootstrap::ExecutionContext::for_workspace(&workspace),
        workspace,
        None,
        tokio::runtime::Runtime::new().unwrap(),
        Globals::default(),
        AppConfig::new_test(),
        None,
        printer,
    );

    let id = ConversationId::try_from(
        DateTime::<Utc>::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000),
    )
    .unwrap();
    let conversation = Conversation {
        title: Some(title.to_owned()),
        ..Default::default()
    };
    ctx.workspace
        .create_conversation_with_id(id, conversation, ctx.config());
    let handle = ctx.workspace.acquire_conversation(&id).unwrap();

    (ctx, out, handle)
}

/// `jp conversation show` with no flags.
fn show() -> Show {
    Show {
        target: PositionalIds::default(),
    }
}

#[test]
fn a_terminal_shows_the_title_without_its_escapes() {
    let (mut ctx, out, handle) = setup("Evil\x1b[2J title", OutputFormat::TextPretty);

    show().run(&mut ctx, vec![handle]).unwrap();
    ctx.printer.flush();
    let raw = out.lock().clone();

    assert!(!raw.contains("\x1b[2J"), "raw: {raw:?}");
    assert!(strip_str(&raw).contains("Evil title"), "raw: {raw:?}");
}

#[test]
fn json_keeps_the_title_as_stored() {
    // Only a terminal is shown the filtered title: a machine reader gets the
    // data.
    let (mut ctx, out, handle) = setup("Evil\x1b[2J title", OutputFormat::Json);

    show().run(&mut ctx, vec![handle]).unwrap();
    ctx.printer.flush();
    let details: serde_json::Value = serde_json::from_str(&out.lock().clone()).unwrap();

    assert_eq!(details["title"], "Evil\u{1b}[2J title");
}
