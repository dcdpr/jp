use std::{fs, sync::Arc};

use camino_tempfile::{Utf8TempDir, tempdir};
use indexmap::IndexMap;
use jp_config::{
    AppConfig, PartialAppConfig,
    conversation::attachment::{AttachmentConfig, AttachmentObjectConfig},
    types::json_value::JsonValue,
    util::build,
};
use jp_conversation::{Conversation, ConversationId};
use jp_printer::{OutputFormat, Printer, SharedBuffer};
use jp_storage::backend::FsStorageBackend;
use jp_workspace::Workspace;
use serde_json::json;
use tokio::runtime::Runtime;
use url::Url;

use super::*;
use crate::{
    Globals,
    cmd::{conversation_id::FlagIds, query::conversation_config},
    config_pipeline::ConfigPipeline,
    ctx::Ctx,
    error::Error,
    parser::AttachmentUrlOrPath,
};

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
    let id = make_id(1_700_000_020);
    let config = AppConfig::new_test();
    let (mut ctx, out, _err, _tmp) =
        persisted_ctx_in(OutputFormat::Json, id, config.clone(), config);

    let handle = ctx.workspace.acquire_conversation(&id).unwrap();
    ls::Ls {
        target: FlagIds::default(),
    }
    .run(&mut ctx, &[handle])
    .unwrap();
    ctx.printer.flush();

    let parsed: serde_json::Value = serde_json::from_str(out.lock().trim()).unwrap();
    assert_eq!(parsed["details"], serde_json::json!([]));
    assert!(
        parsed.get("message").is_none(),
        "an outcome sentence would mean the prose shortcut ran: {parsed}"
    );
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
    // Include an exact duplicate and an alternate `file://` spelling where
    // `Url::parse` places `docs` in the host component.
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

fn persisted_ctx(
    id: ConversationId,
    config: AppConfig,
) -> (Ctx, SharedBuffer, SharedBuffer, Utf8TempDir) {
    persisted_ctx_in(OutputFormat::Text, id, config.clone(), config)
}

/// Build a `Ctx` over a persisted workspace where the invocation runs with
/// `invocation` config and the target conversation was created with `stored`
/// config.
fn persisted_ctx_in(
    format: OutputFormat,
    id: ConversationId,
    stored: AppConfig,
    invocation: AppConfig,
) -> (Ctx, SharedBuffer, SharedBuffer, Utf8TempDir) {
    let tmp = tempdir().unwrap();
    let storage = tmp.path().join(".jp");
    fs::create_dir_all(&storage).unwrap();
    let user = tmp.path().join("user");

    let fs = FsStorageBackend::new(&storage)
        .unwrap()
        .with_user_storage(&user, None, "abc")
        .unwrap();
    let fs = Arc::new(fs);
    let mut workspace = Workspace::in_memory(tmp.path()).with_backend(fs.clone());
    workspace.load_conversation_index();
    workspace.create_conversation_with_id(id, Conversation::default(), stored.into());

    let (printer, out, err) = Printer::memory(format);
    let ctx = Ctx::new(
        crate::bootstrap::ExecutionContext::for_workspace(&workspace),
        workspace,
        Some(fs),
        Runtime::new().unwrap(),
        Globals::default(),
        invocation,
        None,
        printer,
    );

    (ctx, out, err, tmp)
}

/// Table-form attachment config matching
/// `.jp/config/knowledge/rfd-priority.toml`.
fn rfd_list_attachment() -> AttachmentConfig {
    let mut params = IndexMap::new();
    params.insert(
        "args".to_owned(),
        JsonValue(json!(["rfd", "list", "--all"])),
    );
    params.insert("description".to_owned(), JsonValue(json!("RFD status")));

    AttachmentConfig::Object(AttachmentObjectConfig {
        kind: "cmd".to_owned(),
        path: "jp".to_owned(),
        params,
    })
}

const RFD_LIST_URL: &str = "cmd://jp?args=rfd&args=list&args=--all&description=RFD+status";
const TERM_URL: &str = "cmd://printenv?args=TERM";

fn url_arg(url: &str) -> AttachmentUrlOrPath {
    AttachmentUrlOrPath::Url(Url::parse(url).unwrap())
}

fn stored_urls(ctx: &Ctx, id: &ConversationId) -> Vec<String> {
    let handle = ctx.workspace.acquire_conversation(id).unwrap();
    let events = ctx.workspace.events(&handle).unwrap();
    events
        .config()
        .unwrap()
        .conversation
        .attachments
        .iter()
        .map(|attachment| attachment.to_url().unwrap().to_string())
        .collect()
}

fn delta_count(ctx: &Ctx, id: &ConversationId) -> usize {
    let handle = ctx.workspace.acquire_conversation(id).unwrap();
    ctx.workspace
        .events(&handle)
        .unwrap()
        .config_deltas()
        .count()
}

#[test]
fn add_records_the_attachment_on_the_conversation() {
    let id = make_id(1_700_000_010);
    let (mut ctx, out, err, _tmp) = persisted_ctx(id, AppConfig::new_test());
    let rt = ctx.handle().clone();

    let handle = ctx.workspace.acquire_conversation(&id).unwrap();
    let add = add::Add {
        attachments: vec![url_arg(TERM_URL)],
        target: FlagIds::default(),
    };
    rt.block_on(add.run(&mut ctx, vec![handle])).unwrap();
    ctx.printer.flush();

    assert_eq!(stored_urls(&ctx, &id), vec![TERM_URL.to_owned()]);
    assert_eq!(
        *out.lock(),
        format!("Added {TERM_URL} to conversation {id}\n")
    );
    assert_eq!(*err.lock(), "");
}

/// Adding an already-attached URL records no delta and reports a message on
/// stderr.
#[test]
fn add_skips_an_attachment_the_conversation_already_has() {
    let id = make_id(1_700_000_011);
    let mut config = AppConfig::new_test();
    config.conversation.attachments = vec![rfd_list_attachment()];
    let (mut ctx, out, err, _tmp) = persisted_ctx(id, config);
    let rt = ctx.handle().clone();
    let before = delta_count(&ctx, &id);

    let handle = ctx.workspace.acquire_conversation(&id).unwrap();
    let add = add::Add {
        attachments: vec![url_arg(RFD_LIST_URL)],
        target: FlagIds::default(),
    };
    rt.block_on(add.run(&mut ctx, vec![handle])).unwrap();
    ctx.printer.flush();

    assert_eq!(stored_urls(&ctx, &id), vec![RFD_LIST_URL.to_owned()]);
    assert_eq!(delta_count(&ctx, &id), before);
    assert_eq!(*out.lock(), "");
    assert_eq!(
        *err.lock(),
        format!("Already attached to conversation {id}: {RFD_LIST_URL}\n")
    );
}

/// The conversation stores the attachments it was created with, including the
/// ones a config file declared, so those are removed the same way.
#[test]
fn rm_removes_an_attachment_a_config_file_declared() {
    let id = make_id(1_700_000_012);
    let mut config = AppConfig::new_test();
    config.conversation.attachments = vec![
        AttachmentConfig::Url(Url::parse(TERM_URL).unwrap()),
        rfd_list_attachment(),
    ];
    let (mut ctx, out, err, _tmp) = persisted_ctx(id, config);
    let rt = ctx.handle().clone();

    let handle = ctx.workspace.acquire_conversation(&id).unwrap();
    let rm = rm::Rm {
        attachments: vec![url_arg(RFD_LIST_URL)],
        target: FlagIds::default(),
    };
    rt.block_on(rm.run(&mut ctx, vec![handle])).unwrap();
    ctx.printer.flush();

    assert_eq!(stored_urls(&ctx, &id), vec![TERM_URL.to_owned()]);
    assert_eq!(
        *out.lock(),
        format!("Removed {RFD_LIST_URL} from conversation {id}\n")
    );
    assert_eq!(*err.lock(), "");
}

/// A conversation's config does not inherit from the config files once it
/// exists, so a list emptied by `rm` stays empty on the next invocation even
/// though the file layer still declares the attachment.
#[test]
fn rm_of_the_last_attachment_is_not_refilled_from_the_file_layer() {
    let id = make_id(1_700_000_013);
    let mut config = AppConfig::new_test();
    config.conversation.attachments = vec![rfd_list_attachment()];
    let (mut ctx, _out, _err, _tmp) = persisted_ctx(id, config.clone());
    let rt = ctx.handle().clone();

    let handle = ctx.workspace.acquire_conversation(&id).unwrap();
    let rm = rm::Rm {
        attachments: vec![url_arg(RFD_LIST_URL)],
        target: FlagIds::default(),
    };
    rt.block_on(rm.run(&mut ctx, vec![handle])).unwrap();

    // The next invocation's pipeline: the file layer still declares the
    // attachment, and the conversation's layer sits on top of it.
    let base = config.to_partial();
    let pipeline = ConfigPipeline::new(&[], None, None, || Ok(base)).unwrap();
    let handle = ctx.workspace.acquire_conversation(&id).unwrap();
    let (conversation, unsets) =
        conversation_config(&ctx.workspace, PartialAppConfig::default(), &handle).unwrap();
    let partial = pipeline
        .partial_with_conversation(conversation, &unsets)
        .unwrap();
    let resolved = build(partial).unwrap();

    assert!(
        resolved.conversation.attachments.is_empty(),
        "got {:?}",
        resolved.conversation.attachments
    );
}

/// The listing is the stored list `rm` acts on, so an attachment only this
/// invocation's config carries (a `--cfg` override, or a reset to the
/// workspace) is not listed.
#[test]
fn ls_lists_the_stored_attachments_not_the_invocation_config() {
    let id = make_id(1_700_000_015);
    let mut stored = AppConfig::new_test();
    stored.conversation.attachments = vec![rfd_list_attachment()];
    let mut invocation = AppConfig::new_test();
    invocation.conversation.attachments =
        vec![AttachmentConfig::Url(Url::parse(TERM_URL).unwrap())];
    let (mut ctx, out, err, _tmp) = persisted_ctx_in(OutputFormat::Json, id, stored, invocation);

    let handle = ctx.workspace.acquire_conversation(&id).unwrap();
    ls::Ls {
        target: FlagIds::default(),
    }
    .run(&mut ctx, &[handle])
    .unwrap();
    ctx.printer.flush();

    let parsed: serde_json::Value = serde_json::from_str(out.lock().trim()).unwrap();
    assert_eq!(
        parsed,
        json!({ "title": "Attachments", "details": [RFD_LIST_URL] })
    );
    assert_eq!(*err.lock(), "");
}

/// A file attachment whose file was deleted is still removed by its path: the
/// stored URL is rebuilt from the path without reading the file.
#[test]
fn rm_removes_a_file_attachment_whose_file_is_gone() {
    let id = make_id(1_700_000_016);
    let (mut ctx, out, err, tmp) = persisted_ctx(id, AppConfig::new_test());
    let rt = ctx.handle().clone();

    let notes = tmp.path().join("notes.md");
    fs::write(&notes, "notes").unwrap();
    let path_arg = || AttachmentUrlOrPath::Path(notes.as_str().into());

    let handle = ctx.workspace.acquire_conversation(&id).unwrap();
    let add = add::Add {
        attachments: vec![path_arg()],
        target: FlagIds::default(),
    };
    rt.block_on(add.run(&mut ctx, vec![handle])).unwrap();
    // `Url` parses `file:notes.md` into this form.
    assert_eq!(stored_urls(&ctx, &id), vec!["file:///notes.md".to_owned()]);

    fs::remove_file(&notes).unwrap();

    let handle = ctx.workspace.acquire_conversation(&id).unwrap();
    let rm = rm::Rm {
        attachments: vec![path_arg()],
        target: FlagIds::default(),
    };
    rt.block_on(rm.run(&mut ctx, vec![handle])).unwrap();
    ctx.printer.flush();

    assert_eq!(stored_urls(&ctx, &id), Vec::<String>::new());
    assert_eq!(
        *out.lock(),
        format!(
            "Added file:///notes.md to conversation {id}\nRemoved file:///notes.md from \
             conversation {id}\n"
        )
    );
    assert_eq!(*err.lock(), "");
}

/// File names holding characters a URL reads as delimiters, and the URL each is
/// stored and reported as.
#[cfg(not(windows))]
const DELIMITER_NAMES: [(&str, &str); 3] = [
    ("reports/Issue #42.md", "file:///reports/Issue%20%2342.md"),
    ("reports/what?.md", "file:///reports/what%3F.md"),
    ("reports/100%25.md", "file:///reports/100%2525.md"),
];

// Windows rejects `?` in paths.
#[cfg(windows)]
const DELIMITER_NAMES: [(&str, &str); 2] = [
    ("reports/Issue #42.md", "file:///reports/Issue%20%2342.md"),
    ("reports/100%25.md", "file:///reports/100%2525.md"),
];

/// A path given on the command line names the same attachment as a `file` table
/// with that `path`, including characters a URL reads as delimiters.
#[test]
fn rm_of_a_bare_path_removes_the_file_table_for_it() {
    let id = make_id(1_700_000_035);
    let mut config = AppConfig::new_test();
    config.conversation.attachments = DELIMITER_NAMES
        .iter()
        .map(|(name, _)| table("file", name, &[]))
        .collect();
    let (mut ctx, out, err, tmp) = persisted_ctx(id, config);
    let rt = ctx.handle().clone();

    // `parse_unchecked` resolves a relative path against the process directory,
    // so the paths are given absolute, under the workspace root. They are built
    // as paths directly: `from_str` reads a Windows drive letter as a URL scheme.
    let handle = ctx.workspace.acquire_conversation(&id).unwrap();
    let rm = rm::Rm {
        attachments: DELIMITER_NAMES
            .iter()
            .map(|(name, _)| AttachmentUrlOrPath::Path(tmp.path().join(name).as_str().into()))
            .collect(),
        target: FlagIds::default(),
    };
    rt.block_on(rm.run(&mut ctx, vec![handle])).unwrap();
    ctx.printer.flush();

    let removed = DELIMITER_NAMES
        .iter()
        .map(|(_, url)| format!("Removed {url} from conversation {id}\n"))
        .collect::<Vec<_>>()
        .concat();
    assert_eq!(stored_urls(&ctx, &id), Vec::<String>::new());
    assert_eq!(*out.lock(), removed);
    assert_eq!(*err.lock(), "");
}

/// Removing an attachment that is not on the conversation records no delta and
/// reports the unmatched URL on stderr.
#[test]
fn rm_reports_an_attachment_the_conversation_does_not_have() {
    let id = make_id(1_700_000_014);
    let mut config = AppConfig::new_test();
    config.conversation.attachments = vec![rfd_list_attachment()];
    let (mut ctx, out, err, _tmp) = persisted_ctx(id, config);
    let rt = ctx.handle().clone();
    let before = delta_count(&ctx, &id);

    let handle = ctx.workspace.acquire_conversation(&id).unwrap();
    let rm = rm::Rm {
        attachments: vec![url_arg(TERM_URL)],
        target: FlagIds::default(),
    };
    rt.block_on(rm.run(&mut ctx, vec![handle])).unwrap();
    ctx.printer.flush();

    assert_eq!(stored_urls(&ctx, &id), vec![RFD_LIST_URL.to_owned()]);
    assert_eq!(delta_count(&ctx, &id), before);
    assert_eq!(*out.lock(), "");
    assert_eq!(
        *err.lock(),
        format!("Not attached to conversation {id}: {TERM_URL}\n")
    );
}

const DIFF_URL: &str = "gh:pull/1/diff";
const REVIEWS_URL: &str = "gh:pull/1/reviews";

/// Attachments are compared by canonical URL, so another spelling of an
/// attached URL, or the same URL given twice, is skipped while the rest are
/// recorded.
#[test]
fn add_skips_another_spelling_of_an_attached_url() {
    let id = make_id(1_700_000_030);
    let mut config = AppConfig::new_test();
    config.conversation.attachments = vec![AttachmentConfig::Url(Url::parse(DIFF_URL).unwrap())];
    let (mut ctx, out, err, _tmp) = persisted_ctx(id, config);
    let rt = ctx.handle().clone();

    let handle = ctx.workspace.acquire_conversation(&id).unwrap();
    let add = add::Add {
        attachments: vec![
            url_arg("gh://dcdpr/jp/pull/1/diff"),
            url_arg(REVIEWS_URL),
            url_arg(REVIEWS_URL),
        ],
        target: FlagIds::default(),
    };
    rt.block_on(add.run(&mut ctx, vec![handle])).unwrap();
    ctx.printer.flush();

    assert_eq!(stored_urls(&ctx, &id), vec![
        DIFF_URL.to_owned(),
        REVIEWS_URL.to_owned()
    ]);
    assert_eq!(
        *out.lock(),
        format!("Added {REVIEWS_URL} to conversation {id}\n")
    );
    assert_eq!(
        *err.lock(),
        format!(
            "Already attached to conversation {id}: gh://dcdpr/jp/pull/1/diff\nAlready attached \
             to conversation {id}: {REVIEWS_URL}\n"
        )
    );
}

/// A config table and a URL that build the same command are one attachment.
#[test]
fn add_skips_a_url_a_table_entry_already_builds() {
    let id = make_id(1_700_000_031);
    let mut config = AppConfig::new_test();
    config.conversation.attachments = vec![AttachmentConfig::Object(AttachmentObjectConfig {
        kind: "cmd".to_owned(),
        path: "printenv".to_owned(),
        params: IndexMap::from([
            ("args".to_owned(), JsonValue(json!(["TERM"]))),
            ("envs".to_owned(), JsonValue(json!(["TERM"]))),
        ]),
    })];
    let (mut ctx, out, err, _tmp) = persisted_ctx(id, config);
    let rt = ctx.handle().clone();
    let before = delta_count(&ctx, &id);

    let spelling = "cmd://printenv?arg=TERM&env=TERM";
    let handle = ctx.workspace.acquire_conversation(&id).unwrap();
    let add = add::Add {
        attachments: vec![url_arg(spelling)],
        target: FlagIds::default(),
    };
    rt.block_on(add.run(&mut ctx, vec![handle])).unwrap();
    ctx.printer.flush();

    assert_eq!(delta_count(&ctx, &id), before);
    assert_eq!(*out.lock(), "");
    assert_eq!(
        *err.lock(),
        format!("Already attached to conversation {id}: {spelling}\n")
    );
}

/// A `file` table keeps its `path` out of the URL host, so its case survives
/// and it matches the `file:<path>` URL for the same file.
#[test]
fn add_skips_the_file_url_of_an_attached_file_table() {
    let id = make_id(1_700_000_034);
    let mut config = AppConfig::new_test();
    config.conversation.attachments = vec![AttachmentConfig::Object(AttachmentObjectConfig {
        kind: "file".to_owned(),
        path: "Docs/Notes.md".to_owned(),
        params: IndexMap::new(),
    })];
    let (mut ctx, out, err, _tmp) = persisted_ctx(id, config);
    let rt = ctx.handle().clone();
    let before = delta_count(&ctx, &id);

    let handle = ctx.workspace.acquire_conversation(&id).unwrap();
    let add = add::Add {
        attachments: vec![url_arg("file:Docs/Notes.md")],
        target: FlagIds::default(),
    };
    rt.block_on(add.run(&mut ctx, vec![handle])).unwrap();
    ctx.printer.flush();

    assert_eq!(delta_count(&ctx, &id), before);
    assert_eq!(*out.lock(), "");
    assert_eq!(
        *err.lock(),
        format!("Already attached to conversation {id}: file:///Docs/Notes.md\n")
    );
}

/// A URL its handler rejects is refused before anything is recorded.
#[test]
fn add_refuses_a_url_its_handler_cannot_resolve() {
    let id = make_id(1_700_000_032);
    let (mut ctx, out, err, _tmp) = persisted_ctx(id, AppConfig::new_test());
    let rt = ctx.handle().clone();
    let before = delta_count(&ctx, &id);

    let bad = "cmd://printenv?env=TERM=xterm";
    let handle = ctx.workspace.acquire_conversation(&id).unwrap();
    let add = add::Add {
        attachments: vec![url_arg(bad)],
        target: FlagIds::default(),
    };
    let error = rt
        .block_on(add.run(&mut ctx, vec![handle]))
        .expect_err("an invalid attachment is refused");
    ctx.printer.flush();

    // `with_cause` lifts the message out of the metadata and lists the cause
    // under an empty key.
    assert_eq!(error.message.as_deref(), Some("Attachment error"));
    assert_eq!(error.metadata, vec![
        ("uri".to_owned(), json!(bad)),
        (
            String::new(),
            json!(
                "invalid `env` value `TERM=xterm`: expected the name of an environment variable, \
                 such as `TERM`"
            )
        ),
    ]);
    assert_eq!(stored_urls(&ctx, &id), Vec::<String>::new());
    assert_eq!(delta_count(&ctx, &id), before);
    assert_eq!(*out.lock(), "");
    assert_eq!(*err.lock(), "");
}

/// `rm` matches by canonical URL too, and reports the stored spelling it
/// removed.
#[test]
fn rm_removes_another_spelling_of_the_url() {
    let id = make_id(1_700_000_033);
    let mut config = AppConfig::new_test();
    config.conversation.attachments = vec![
        AttachmentConfig::Url(Url::parse(DIFF_URL).unwrap()),
        AttachmentConfig::Url(Url::parse(REVIEWS_URL).unwrap()),
    ];
    let (mut ctx, out, err, _tmp) = persisted_ctx(id, config);
    let rt = ctx.handle().clone();

    let handle = ctx.workspace.acquire_conversation(&id).unwrap();
    let rm = rm::Rm {
        attachments: vec![url_arg("gh://dcdpr/jp/pull/1/diff")],
        target: FlagIds::default(),
    };
    rt.block_on(rm.run(&mut ctx, vec![handle])).unwrap();
    ctx.printer.flush();

    assert_eq!(stored_urls(&ctx, &id), vec![REVIEWS_URL.to_owned()]);
    assert_eq!(
        *out.lock(),
        format!("Removed {DIFF_URL} from conversation {id}\n")
    );
    assert_eq!(*err.lock(), "");
}

const RETIRED_MCP_URL: &str = "mcp+github-mcp-server+repo://owner/name";

/// A retired MCP resource URI is refused before anything is recorded, since a
/// stored one fails every query on the conversation.
#[test]
fn add_refuses_a_retired_mcp_attachment() {
    let id = make_id(1_700_000_017);
    let (mut ctx, out, err, _tmp) = persisted_ctx(id, AppConfig::new_test());
    let rt = ctx.handle().clone();
    let before = delta_count(&ctx, &id);

    let handle = ctx.workspace.acquire_conversation(&id).unwrap();
    let add = add::Add {
        attachments: vec![url_arg(RETIRED_MCP_URL)],
        target: FlagIds::default(),
    };
    let error = rt
        .block_on(add.run(&mut ctx, vec![handle]))
        .expect_err("a retired attachment is refused");
    ctx.printer.flush();

    // `with_cause` lifts the message out of the metadata and lists the cause
    // under an empty key.
    assert_eq!(error.message.as_deref(), Some("Attachment error"));
    assert_eq!(error.metadata, vec![
        ("uri".to_owned(), json!(RETIRED_MCP_URL)),
        (
            String::new(),
            json!(format!(
                "MCP resource attachments are retired and can no longer be added: \
                 `{RETIRED_MCP_URL}`"
            ))
        ),
    ]);
    assert_eq!(stored_urls(&ctx, &id), Vec::<String>::new());
    assert_eq!(delta_count(&ctx, &id), before);
    assert_eq!(*out.lock(), "");
    assert_eq!(*err.lock(), "");
}

/// A conversation that already carries a retired MCP URI can still drop it,
/// which is what the handler's error tells the user to do.
#[test]
fn rm_removes_a_retired_mcp_attachment() {
    let id = make_id(1_700_000_018);
    let mut config = AppConfig::new_test();
    config.conversation.attachments =
        vec![AttachmentConfig::Url(Url::parse(RETIRED_MCP_URL).unwrap())];
    let (mut ctx, out, err, _tmp) = persisted_ctx(id, config);
    let rt = ctx.handle().clone();

    let handle = ctx.workspace.acquire_conversation(&id).unwrap();
    let rm = rm::Rm {
        attachments: vec![url_arg(RETIRED_MCP_URL)],
        target: FlagIds::default(),
    };
    rt.block_on(rm.run(&mut ctx, vec![handle])).unwrap();
    ctx.printer.flush();

    assert_eq!(stored_urls(&ctx, &id), Vec::<String>::new());
    assert_eq!(
        *out.lock(),
        format!("Removed {RETIRED_MCP_URL} from conversation {id}\n")
    );
    assert_eq!(*err.lock(), "");
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

fn table(kind: &str, path: &str, params: &[(&str, &str)]) -> AttachmentConfig {
    AttachmentConfig::Object(AttachmentObjectConfig {
        kind: kind.to_owned(),
        path: path.to_owned(),
        params: params
            .iter()
            .map(|(k, v)| ((*k).to_owned(), JsonValue((*v).into())))
            .collect::<IndexMap<_, _>>(),
    })
}

/// A `file` table resolves to the URL `jp attachment add <path>` writes, so the
/// two forms name the same attachment and `rm` finds either.
#[test]
fn a_file_table_and_its_file_url_are_the_same_attachment() {
    let cases = [
        ("Docs/Notes.md", &[][..], "file:Docs/Notes.md"),
        ("café/my notes.md", &[][..], "file:café/my notes.md"),
        (
            "**/*.md",
            &[("exclude", "true")][..],
            "file:**/*.md?exclude=true",
        ),
    ];

    for (path, params, written) in cases {
        assert_eq!(
            attachment_url(&table("file", path, params)).unwrap(),
            Url::parse(written).unwrap(),
            "{path}"
        );
    }

    assert_eq!(
        attachment_url(&table("file", "Docs/Notes.md", &[]))
            .unwrap()
            .as_str(),
        "file:///Docs/Notes.md"
    );
}

/// A type whose handler keeps the default builds `<type>://<path>`, as does a
/// type with no handler.
#[test]
fn other_tables_build_the_generic_url() {
    assert_eq!(
        attachment_url(&table("https", "example.com/docs", &[("lang", "en")]))
            .unwrap()
            .as_str(),
        "https://example.com/docs?lang=en"
    );
    assert_eq!(
        attachment_url(&table("jp", "jp-c123456", &[("raw", "")]))
            .unwrap()
            .as_str(),
        "jp://jp-c123456?raw="
    );
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
