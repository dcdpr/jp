use std::{fs, time::Duration};

use camino_tempfile::tempdir;
use chrono::{DateTime, Utc};
use jp_config::{AppConfig, PartialAppConfig, assignment::KvAssignment};
use jp_conversation::{Conversation, ConversationId};
use jp_printer::{OutputFormat, Printer};
use jp_workspace::Workspace;
use tokio::runtime::Runtime;

use super::*;
use crate::{Globals, KeyValueOrPath, cmd::conversation_id::FlagIds, ctx::Ctx};

fn make_id(secs: u64) -> ConversationId {
    ConversationId::try_from(DateTime::<Utc>::UNIX_EPOCH + Duration::from_secs(secs)).unwrap()
}

fn kv(s: &str) -> KeyValueOrPath {
    KeyValueOrPath::KeyValue(s.parse::<KvAssignment>().unwrap())
}

/// Set up a Ctx with a persisted workspace and optional conversations.
fn setup(
    cfg_args: Vec<KeyValueOrPath>,
    conversation_ids: &[ConversationId],
) -> (Ctx, camino_tempfile::Utf8TempDir) {
    let tmp = tempdir().unwrap();
    let storage = tmp.path().join(".jp");
    fs::create_dir_all(&storage).unwrap();
    let user = tmp.path().join("user");

    let config = AppConfig::new_test();
    let mut workspace = Workspace::new(tmp.path())
        .persisted_at(&storage)
        .unwrap()
        .with_local_storage_at(&user, "test", "abc")
        .unwrap();
    workspace.load_conversation_index();

    for &id in conversation_ids {
        workspace.create_conversation_with_id(id, Conversation::default(), config.clone().into());
    }

    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    let globals = Globals {
        config: cfg_args,
        ..Default::default()
    };
    let ctx = Ctx::new(
        workspace,
        Runtime::new().unwrap(),
        globals,
        config,
        None,
        printer,
    );

    (ctx, tmp)
}

#[test]
fn build_partial_errors_when_no_args() {
    let base = PartialAppConfig::default();
    let result = crate::config_pipeline::build_partial_from_cfg_args(&[], &base, None);
    assert!(result.is_err());
}

#[test]
fn build_partial_applies_key_value() {
    let base = PartialAppConfig::default();
    let args = vec![kv("conversation.start_local=true")];
    let partial = crate::config_pipeline::build_partial_from_cfg_args(&args, &base, None).unwrap();
    assert_eq!(partial.conversation.start_local, Some(true));
}

#[test]
fn build_partial_applies_multiple_key_values() {
    let base = PartialAppConfig::default();
    let args = vec![
        kv("conversation.start_local=true"),
        kv("conversation.default_id=last"),
    ];
    let partial = crate::config_pipeline::build_partial_from_cfg_args(&args, &base, None).unwrap();
    assert_eq!(partial.conversation.start_local, Some(true));
    assert_eq!(
        partial.conversation.default_id,
        Some(jp_config::conversation::DefaultConversationId::LastActivated)
    );
}

#[test]
fn build_partial_loads_toml_file() {
    let tmp = tempdir().unwrap();
    let file = tmp.path().join("test.toml");
    fs::write(&file, "[conversation]\nstart_local = true\n").unwrap();

    let base = PartialAppConfig::default();
    let args = vec![KeyValueOrPath::Path(file)];
    let partial = crate::config_pipeline::build_partial_from_cfg_args(&args, &base, None).unwrap();
    assert_eq!(partial.conversation.start_local, Some(true));
}

#[test]
fn build_partial_merges_file_and_kv() {
    let tmp = tempdir().unwrap();
    let file = tmp.path().join("base.toml");
    fs::write(&file, "[conversation]\nstart_local = true\n").unwrap();

    let base = PartialAppConfig::default();
    let args = vec![
        KeyValueOrPath::Path(file),
        kv("conversation.default_id=last"),
    ];
    let partial = crate::config_pipeline::build_partial_from_cfg_args(&args, &base, None).unwrap();
    assert_eq!(partial.conversation.start_local, Some(true));
    assert_eq!(
        partial.conversation.default_id,
        Some(jp_config::conversation::DefaultConversationId::LastActivated)
    );
}

#[test]
fn build_partial_errors_on_missing_file() {
    let base = PartialAppConfig::default();
    let args = vec![KeyValueOrPath::Path("/nonexistent/path.toml".into())];
    let result = crate::config_pipeline::build_partial_from_cfg_args(&args, &base, None);
    assert!(result.is_err());
}

#[test]
fn set_in_conversation_applies_config_delta() {
    let id = make_id(1000);
    let (mut ctx, _tmp) = setup(vec![kv("conversation.start_local=true")], &[id]);
    let rt = ctx.handle().clone();

    let handle = ctx.workspace.acquire_conversation(&id).unwrap();
    let set = Set {
        file_target: FileTarget::default(),
        conversation: FlagIds::default(),
    };
    rt.block_on(set.run(&mut ctx, vec![handle])).unwrap();

    let handle = ctx.workspace.acquire_conversation(&id).unwrap();
    let events = ctx.workspace.events(&handle).unwrap();
    let config = events.config().unwrap();
    assert!(config.conversation.start_local);
}

#[test]
fn set_in_multiple_conversations() {
    let id1 = make_id(1000);
    let id2 = make_id(2000);
    let (mut ctx, _tmp) = setup(vec![kv("conversation.start_local=true")], &[id1, id2]);
    let rt = ctx.handle().clone();

    let h1 = ctx.workspace.acquire_conversation(&id1).unwrap();
    let h2 = ctx.workspace.acquire_conversation(&id2).unwrap();
    let set = Set {
        file_target: FileTarget::default(),
        conversation: FlagIds::default(),
    };
    rt.block_on(set.run(&mut ctx, vec![h1, h2])).unwrap();

    for id in [id1, id2] {
        let handle = ctx.workspace.acquire_conversation(&id).unwrap();
        let events = ctx.workspace.events(&handle).unwrap();
        let config = events.config().unwrap();
        assert!(config.conversation.start_local);
    }
}

#[test]
fn set_in_workspace_file() {
    let (mut ctx, tmp) = setup(vec![kv("conversation.start_local=true")], &[]);
    let rt = ctx.handle().clone();

    // ConfigLoader expects a file to exist.
    let config_path = tmp.path().join(".jp/config.toml");
    fs::write(&config_path, "").unwrap();

    let set = Set {
        file_target: FileTarget::default(),
        conversation: FlagIds::default(),
    };
    rt.block_on(set.run(&mut ctx, vec![])).unwrap();

    let content = fs::read_to_string(config_path).unwrap();
    assert!(
        content.contains("start_local = true"),
        "config file should contain the set value: {content}"
    );
}

#[test]
fn set_multiple_values_in_file() {
    let (mut ctx, tmp) = setup(
        vec![
            kv("conversation.start_local=true"),
            kv("conversation.default_id=last"),
        ],
        &[],
    );
    let rt = ctx.handle().clone();

    let config_path = tmp.path().join(".jp/config.toml");
    fs::write(&config_path, "").unwrap();

    let set = Set {
        file_target: FileTarget::default(),
        conversation: FlagIds::default(),
    };
    rt.block_on(set.run(&mut ctx, vec![])).unwrap();

    let content = fs::read_to_string(config_path).unwrap();
    assert!(content.contains("start_local = true"), "got: {content}");
    assert!(
        content.contains("default_id") && content.contains("last"),
        "got: {content}"
    );
}

#[test]
fn set_from_toml_file_into_conversation() {
    let id = make_id(3000);
    let tmp_cfg = tempdir().unwrap();
    let cfg_file = tmp_cfg.path().join("profile.toml");
    fs::write(&cfg_file, "[conversation]\nstart_local = true\n").unwrap();

    let (mut ctx, _tmp) = setup(vec![KeyValueOrPath::Path(cfg_file)], &[id]);
    let rt = ctx.handle().clone();

    let handle = ctx.workspace.acquire_conversation(&id).unwrap();
    let set = Set {
        file_target: FileTarget::default(),
        conversation: FlagIds::default(),
    };
    rt.block_on(set.run(&mut ctx, vec![handle])).unwrap();

    let handle = ctx.workspace.acquire_conversation(&id).unwrap();
    let events = ctx.workspace.events(&handle).unwrap();
    let config = events.config().unwrap();
    assert!(config.conversation.start_local);
}

#[test]
fn set_errors_without_cfg_args() {
    let (mut ctx, _tmp) = setup(vec![], &[]);
    let rt = ctx.handle().clone();

    let set = Set {
        file_target: FileTarget::default(),
        conversation: FlagIds::default(),
    };
    let result = rt.block_on(set.run(&mut ctx, vec![]));
    assert!(result.is_err());
}

#[test]
fn set_in_file_preserves_existing_formatting() {
    let (mut ctx, tmp) = setup(vec![kv("conversation.default_id=ask")], &[]);
    let rt = ctx.handle().clone();

    let config_path = tmp.path().join(".jp/config.toml");
    let original = indoc::indoc! {r#"
        extends = [
            "config/personas/default.toml",
            "mcp/tools/**/*.toml",
        ]

        [providers.llm.aliases]
        anthropic = "anthropic/claude-sonnet-4-6"
        haiku = "anthropic/claude-haiku-4-5"

        [style.code]
        copy_link = "osc8"
    "#};
    fs::write(&config_path, original).unwrap();

    let set = Set {
        file_target: FileTarget::default(),
        conversation: FlagIds::default(),
    };
    rt.block_on(set.run(&mut ctx, vec![])).unwrap();

    let content = fs::read_to_string(&config_path).unwrap();

    // The delta should be applied
    assert!(
        content.contains(r#"default_id = "ask""#),
        "delta applied: {content}"
    );

    // Existing content should be preserved exactly
    assert!(
        content.contains(r#"anthropic = "anthropic/claude-sonnet-4-6""#),
        "aliases should remain as compact strings: {content}"
    );
    assert!(
        content.contains("extends = ["),
        "extends array preserved: {content}"
    );
    assert!(
        content.contains(r#"copy_link = "osc8""#),
        "style preserved: {content}"
    );
}

#[test]
fn load_request_none_when_no_ids() {
    let set = Set {
        file_target: FileTarget::default(),
        conversation: FlagIds::default(),
    };
    let req = set.conversation_load_request();
    assert!(req.targets.is_none());
}

#[test]
fn load_request_explicit_when_ids_present() {
    let set = Set {
        file_target: FileTarget::default(),
        conversation: FlagIds {
            ids: vec![crate::cmd::target::ConversationTarget::LastActivated],
        },
    };
    let req = set.conversation_load_request();
    assert!(req.targets.is_some());
    assert_eq!(req.targets.unwrap().len(), 1);
}
