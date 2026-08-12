use camino_tempfile::tempdir;
use jp_config::AppConfig;
use jp_conversation::ConversationId;
use jp_printer::{OutputFormat, Printer};
use jp_workspace::Workspace;
use tokio::runtime::Runtime;
use url::Url;

use super::*;
use crate::{Globals, ctx::Ctx, error::Error};

fn make_id(secs: u64) -> ConversationId {
    ConversationId::try_from(
        chrono::DateTime::<chrono::Utc>::UNIX_EPOCH + std::time::Duration::from_secs(secs),
    )
    .unwrap()
}

/// Build a `Ctx` whose workspace has no conversations indexed.
/// All `jp://` resolutions against this Ctx therefore exercise the
/// missing-conversation path.
fn empty_ctx() -> (Ctx, Runtime) {
    let tmp = tempdir().unwrap();
    let workspace = Workspace::new(tmp.path().to_path_buf());

    let (printer, _out, _err) = Printer::memory(OutputFormat::Text);
    let runtime = Runtime::new().unwrap();
    let ctx = Ctx::new(
        crate::bootstrap::ExecutionContext::for_workspace(&workspace),
        workspace,
        None,
        Runtime::new().unwrap(),
        Globals::default(),
        AppConfig::new_test(),
        None,
        printer,
    );

    (ctx, runtime)
}

#[test]
fn register_attachment_returns_typed_missing_for_unknown_conversation() {
    let (ctx, runtime) = empty_ctx();
    let id = make_id(1_700_000_001);
    let uri = Url::parse(&format!("jp://{id}")).unwrap();

    let err = runtime
        .block_on(register_attachment(&ctx, uri.clone()))
        .expect_err("expected the missing-conversation variant");

    match err {
        Error::AttachmentConversationMissing {
            id: missing_id,
            uri: missing_uri,
        } => {
            assert_eq!(missing_id, id);
            assert_eq!(missing_uri, uri);
        }
        other => panic!("expected AttachmentConversationMissing, got {other:?}"),
    }
}

#[test]
fn load_conversation_attachments_skips_missing_references() {
    let (ctx, runtime) = empty_ctx();
    let first = Url::parse(&format!("jp://{}", make_id(1_700_000_002))).unwrap();
    let second = Url::parse(&format!("jp://{}", make_id(1_700_000_003))).unwrap();

    let attachments = runtime
        .block_on(load_conversation_attachments(&ctx, vec![first, second]))
        .expect("missing references should not propagate as errors");

    // Both URLs point at conversations the workspace doesn't know about, so
    // both get warn-and-skipped. The query continues with zero attachments.
    assert!(
        attachments.is_empty(),
        "got {} attachments",
        attachments.len()
    );
}

#[test]
fn load_conversation_attachments_propagates_other_errors() {
    let (ctx, runtime) = empty_ctx();
    let id = make_id(1_700_000_004);
    // An invalid selector fails before the workspace lookup runs, so the
    // error is structural (not a missing-conversation condition) and must
    // surface to the caller.
    let bad_uri = Url::parse(&format!("jp://{id}?select=zzz")).unwrap();

    let err = runtime
        .block_on(load_conversation_attachments(&ctx, vec![bad_uri.clone()]))
        .expect_err("structural attachment errors should not be silently skipped");

    match err {
        Error::AttachmentFailed { uri, .. } => assert_eq!(uri, bad_uri),
        other => panic!("expected AttachmentFailed, got {other:?}"),
    }
}

#[test]
fn register_attachment_failure_carries_uri_and_source() {
    let (ctx, runtime) = empty_ctx();
    // The `cmd` handler accepts the URI at `add` time and only spawns the
    // binary at `get` time, so a missing binary surfaces as a handler failure
    // carrying both the URI and the underlying spawn error.
    let uri = Url::parse("cmd://jp-definitely-missing-binary").unwrap();

    let err = runtime
        .block_on(register_attachment(&ctx, uri.clone()))
        .expect_err("spawning a missing binary should fail");

    match err {
        Error::AttachmentFailed {
            uri: failed_uri,
            source,
        } => {
            assert_eq!(failed_uri, uri);
            assert!(
                source.to_string().contains("jp-definitely-missing-binary"),
                "source should name the failing command, got: {source}"
            );
        }
        other => panic!("expected AttachmentFailed, got {other:?}"),
    }
}
