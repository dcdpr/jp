use std::fs;

use camino_tempfile::tempdir;
use indexmap::IndexMap;
use jp_config::{
    AppConfig,
    conversation::attachment::{PartialAttachmentConfig, PartialAttachmentObjectConfig},
    types::json_value::JsonValue,
};
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
    let workspace = Workspace::in_memory(tmp.path().to_path_buf());

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

/// A listing's JSON shape is a property of the command, not of whether this
/// particular conversation happens to have anything attached.
/// The empty case previously took a `println` shortcut, which wraps prose in a
/// `{"message": ...}` envelope and left a consumer with no `details` at all.
#[test]
fn an_empty_listing_is_still_an_array_in_json() {
    let tmp = tempdir().unwrap();
    let workspace = Workspace::in_memory(tmp.path().to_path_buf());
    let (printer, out, _err) = Printer::memory(OutputFormat::Json);
    let mut ctx = Ctx::new(
        crate::bootstrap::ExecutionContext::for_workspace(&workspace),
        workspace,
        None,
        Runtime::new().unwrap(),
        Globals::default(),
        AppConfig::new_test(),
        None,
        printer,
    );

    assert!(
        ctx.config().conversation.attachments.is_empty(),
        "the test config must carry no attachments"
    );

    ls::Ls {}.run(&mut ctx).unwrap();
    ctx.printer.flush();

    let parsed: serde_json::Value = serde_json::from_str(out.lock().trim()).unwrap();
    assert_eq!(parsed["details"], serde_json::json!([]));
    assert!(
        parsed.get("message").is_none(),
        "an outcome sentence would mean the prose shortcut ran: {parsed}"
    );
}

fn add_command(args: &[&str]) -> add::Add {
    let matches = add::Add::augment(Command::new("add"))
        .try_get_matches_from(["add"].iter().chain(args))
        .unwrap();

    add::Add::from_matches(&matches).unwrap()
}

#[test]
fn attachment_add_skips_a_url_already_attached() {
    let diff = Url::parse("gh:pull/1/diff").unwrap();
    let reviews = Url::parse("gh:pull/1/reviews").unwrap();
    let mut partial = PartialAppConfig::default();
    partial.conversation.attachments.push(diff.clone().into());

    let partial = add_command(&["gh:pull/1/diff", "gh:pull/1/reviews"])
        .apply_cli_config(None, partial, None)
        .unwrap();

    assert_eq!(partial.conversation.attachments.to_vec(), vec![
        PartialAttachmentConfig::Url(diff),
        PartialAttachmentConfig::Url(reviews),
    ]);
}

#[test]
fn attachment_add_keeps_one_entry_for_a_url_given_twice() {
    let diff = Url::parse("gh:pull/1/diff").unwrap();

    let partial = add_command(&["gh:pull/1/diff", "gh:pull/1/diff"])
        .apply_cli_config(None, PartialAppConfig::default(), None)
        .unwrap();

    assert_eq!(partial.conversation.attachments.to_vec(), vec![
        PartialAttachmentConfig::Url(diff)
    ]);
}

#[test]
fn attachment_add_skips_a_url_a_table_entry_already_builds() {
    let table = PartialAttachmentConfig::Object(PartialAttachmentObjectConfig {
        kind: Some("cmd".to_owned()),
        path: Some("date".to_owned()),
        params: None,
    });
    let mut partial = PartialAppConfig::default();
    partial.conversation.attachments.push(table.clone());

    let partial = add_command(&["cmd://date"])
        .apply_cli_config(None, partial, None)
        .unwrap();

    assert_eq!(partial.conversation.attachments.to_vec(), vec![table]);
}

/// A config table writes `params.args` and `params.envs`, which become `args=`
/// and `envs=`; the README's CLI example writes `arg=` and `env=`.
/// Both run the same command.
#[test]
fn attachment_add_skips_another_spelling_of_an_attached_url() {
    let table = PartialAttachmentConfig::Object(PartialAttachmentObjectConfig {
        kind: Some("cmd".to_owned()),
        path: Some("printenv".to_owned()),
        params: Some(IndexMap::from([
            ("args".to_owned(), JsonValue(serde_json::json!(["TERM"]))),
            ("envs".to_owned(), JsonValue(serde_json::json!(["TERM"]))),
        ])),
    });
    let mut partial = PartialAppConfig::default();
    partial.conversation.attachments.push(table.clone());

    let partial = add_command(&["cmd://printenv?arg=TERM&env=TERM"])
        .apply_cli_config(None, partial, None)
        .unwrap();

    assert_eq!(partial.conversation.attachments.to_vec(), vec![table]);
}

/// The URL is rejected before anything is attached, rather than when the next
/// query resolves it.
#[test]
fn attachment_add_rejects_a_url_its_handler_cannot_resolve() {
    let error = add_command(&["cmd://printenv?env=TERM=xterm"])
        .apply_cli_config(None, PartialAppConfig::default(), None)
        .unwrap_err();

    assert_eq!(
        crate::error::error_chain(error.as_ref()),
        "Attachment error for cmd://printenv?env=TERM=xterm: invalid `env` value `TERM=xterm`: \
         expected the name of an environment variable, such as `TERM`"
    );
}

fn rm_command(args: &[&str]) -> rm::Rm {
    let matches = rm::Rm::augment(Command::new("rm"))
        .try_get_matches_from(["rm"].iter().chain(args))
        .unwrap();

    rm::Rm::from_matches(&matches).unwrap()
}

#[test]
fn attachment_rm_removes_another_spelling_of_the_url() {
    let reviews = Url::parse("gh:pull/1/reviews").unwrap();
    let mut partial = PartialAppConfig::default();
    partial
        .conversation
        .attachments
        .push(Url::parse("gh:pull/1/diff").unwrap().into());
    partial
        .conversation
        .attachments
        .push(reviews.clone().into());

    let partial = rm_command(&["gh://dcdpr/jp/pull/1/diff"])
        .apply_cli_config(None, partial, None)
        .unwrap();

    assert_eq!(partial.conversation.attachments.to_vec(), vec![
        PartialAttachmentConfig::Url(reviews)
    ]);
}

#[test]
fn load_conversation_attachments_resolves_a_repeated_url_once() {
    let tmp = tempdir().unwrap();
    fs::create_dir(tmp.path().join("docs")).unwrap();
    fs::write(tmp.path().join("docs/notes.md"), "hello").unwrap();
    let workspace = Workspace::in_memory(tmp.path().to_path_buf());
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
    // The same URL twice, and the same file spelled another way: `docs` is the
    // host of the last one, which the parser keeps apart from the path.
    let urls = [
        "file:docs/notes.md",
        "file:docs/notes.md",
        "file://docs/notes.md",
    ]
    .map(|url| Url::parse(url).unwrap())
    .to_vec();

    let groups = runtime
        .block_on(load_conversation_attachments(&ctx, urls))
        .unwrap();

    let sources: Vec<Vec<&str>> = groups
        .iter()
        .map(|group| group.iter().map(|a| a.source.as_str()).collect())
        .collect();
    assert_eq!(sources, [vec!["docs/notes.md"], vec![], vec![]]);
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

    let groups = runtime
        .block_on(load_conversation_attachments(&ctx, vec![first, second]))
        .expect("missing references should not propagate as errors");

    // Both URLs point at conversations the workspace doesn't know about, so
    // both get warn-and-skipped. A skipped reference still holds its slot, so
    // the groups stay aligned with the URLs and carry no attachments.
    assert_eq!(groups.len(), 2);
    assert!(groups.iter().all(Vec::is_empty), "got {groups:?}");
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
