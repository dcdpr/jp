use std::{io, sync::Arc};

use assert_matches::assert_matches;
use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::Utf8TempDir;
use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use inquire::InquireError;
use jp_config::{
    AppConfig, PartialAppConfig,
    assistant::tool_choice::ToolChoice,
    conversation::tool::{AllowToggle, Enable, PartialEnableConfig, PartialToolConfig},
    model::id::{PartialModelIdConfig, ProviderId},
    types::command::CommandConfigOrString,
    util::build,
};
use jp_conversation::{
    Conversation, ConversationEvent, ConversationId,
    event::{ChatRequest, ChatResponse},
};
use jp_inquire::{InlineOption, ReplyEditMode, ReplyOutcome, prompt::MockPromptBackend};
use jp_printer::{OutputFormat, Printer, SharedBuffer};
use jp_storage::{
    backend::{ConversationFilter, FsStorageBackend, LoadBackend},
    load::projected_conversation_ids,
};
use jp_workspace::{
    ConversationHandle, Workspace,
    session::{Session, SessionId, SessionSource},
};
use serde_json::Value;
use tokio::runtime::Runtime;

use super::*;
use crate::{
    Cli, Globals,
    bootstrap::ExecutionContext,
    cli::flag::{ArgPrompts, Parsed, SettleError},
    cmd::{
        Commands,
        compact_flag::CompactSpec,
        target::{ConversationTarget, PickerFilter},
    },
    ctx::IntoPartialAppConfig,
    editor, resolve_config,
};

fn make_partial_with_tools() -> PartialAppConfig {
    let mut partial = PartialAppConfig::default();
    *partial.conversation.tools.tools = IndexMap::from_iter([
        ("implicitly_enabled_tool".into(), PartialToolConfig {
            enable: None,
            ..Default::default()
        }),
        ("explicitly_enabled_tool".into(), PartialToolConfig {
            enable: Some(PartialEnableConfig::ON),
            ..Default::default()
        }),
        ("explicitly_disabled_tool".into(), PartialToolConfig {
            enable: Some(PartialEnableConfig::OFF),
            ..Default::default()
        }),
        ("explicit_tool".into(), PartialToolConfig {
            enable: Some(PartialEnableConfig {
                state: Some(false),
                allow_toggle: Some(AllowToggle::IfNamed),
            }),
            ..Default::default()
        }),
    ]);
    partial
}

/// Resolve a tool's effective [`Enable`] from a partial config: the per-tool
/// value over the global `*` defaults, then the hardcoded fallback.
fn effective(partial: &PartialAppConfig, name: &str) -> Enable {
    let defaults = partial
        .conversation
        .tools
        .defaults
        .enable
        .clone()
        .unwrap_or_default();
    partial.conversation.tools.tools[name]
        .enable
        .clone()
        .unwrap_or_default()
        .effective(&defaults)
}

fn make_id(secs: u64) -> ConversationId {
    ConversationId::try_from(DateTime::<Utc>::UNIX_EPOCH + std::time::Duration::from_secs(secs))
        .unwrap()
}

fn config_with_model(provider: ProviderId, name: &str) -> AppConfig {
    let mut partial = AppConfig::new_test().to_partial();
    partial.assistant.model.id = PartialModelIdConfig {
        provider: Some(provider),
        name: Some(name.parse().unwrap()),
    }
    .into();

    build(partial).unwrap()
}

#[test]
fn tier_flag_parses_every_rung() {
    for (arg, expected) in [
        ("off", ServiceTier::Off),
        ("flex", ServiceTier::Flex),
        ("standard", ServiceTier::Standard),
        ("priority", ServiceTier::Priority),
    ] {
        let query = parse_query(&["--tier", arg]).unwrap();
        assert_eq!(query.service_tier, Some(expected));
    }

    // Rungs individual providers offer that JP does not model; accepting either
    // would hand an unmappable value to every other provider.
    assert!(parse_query(&["--tier", "auto"]).is_err());
    assert!(parse_query(&["--tier", "scale"]).is_err());
}

#[test]
fn tier_flag_reaches_the_model_parameters() {
    let query = parse_query(&["--tier", "flex"]).unwrap();

    let partial = query
        .apply_cli_config(None, PartialAppConfig::empty(), None)
        .unwrap();

    assert_eq!(
        partial.assistant.model.parameters.service_tier,
        Some(ServiceTier::Flex)
    );
}

#[test]
fn tier_and_no_tier_flags_conflict() {
    assert!(parse_query(&["--tier", "flex", "--no-tier"]).is_err());
}

#[test]
fn resolve_new_title_uses_leading_heading() {
    assert_eq!(
        resolve_new_title(true, true, "# Fix the parser\n\nbody text"),
        NewTitle::FromHeading("Fix the parser".to_owned())
    );
}

#[test]
fn resolve_new_title_heading_wins_when_generation_disabled() {
    // The two flags are independent: `from_heading` still applies even
    // with LLM generation turned off.
    assert_eq!(
        resolve_new_title(true, false, "# Title"),
        NewTitle::FromHeading("Title".to_owned())
    );
}

#[test]
fn resolve_new_title_disabled_heading_falls_through_to_generation() {
    assert_eq!(
        resolve_new_title(false, true, "# Title"),
        NewTitle::Generate
    );
}

#[test]
fn resolve_new_title_no_heading_generates() {
    assert_eq!(
        resolve_new_title(true, true, "just a plain prompt"),
        NewTitle::Generate
    );
}

#[test]
fn resolve_new_title_skips_when_both_disabled() {
    assert_eq!(resolve_new_title(false, false, "# Title"), NewTitle::Skip);
    assert_eq!(
        resolve_new_title(true, false, "no heading here"),
        NewTitle::Skip
    );
}

#[test]
fn resolve_title_override_no_title_clears_inherited_title() {
    // `--no-title` should clear an inherited title (the
    // `--fork --no-title` case from PR #600 review): a forked
    // conversation inherits the source's title via
    // `fork_conversation`, and `--no-title` is supposed to leave
    // the run with no title at all.
    assert_eq!(
        resolve_title_override(Some("inherited".to_owned()), None, true),
        None
    );
}

#[test]
fn resolve_title_override_no_title_clears_resumed_title() {
    // `--no-title` is symmetric with `--title T`: both name what
    // `metadata.title` ends up as, regardless of whether the
    // conversation is new, forked, or resumed.
    assert_eq!(
        resolve_title_override(Some("existing".to_owned()), None, true),
        None
    );
}

#[test]
fn resolve_title_override_title_overwrites_existing_title() {
    assert_eq!(
        resolve_title_override(Some("old".to_owned()), Some("new"), false),
        Some("new".to_owned())
    );
}

#[test]
fn resolve_title_override_neither_flag_keeps_the_stored_title() {
    assert_eq!(
        resolve_title_override(Some("keep".to_owned()), None, false),
        Some("keep".to_owned())
    );
    assert_eq!(resolve_title_override(None, None, false), None);
}

#[test]
fn no_title_does_not_persist_into_partial_config() {
    // Regression for the persistence concern in PR #600: routing
    // `--no-title` through `apply_cli_config` previously wrote
    // `conversation.title.generate.auto = Some(false)` into the
    // partial, which would then flow into the conversation's
    // `config_delta` via `turn_config_delta` and persist
    // for every future query on that conversation. The flag is
    // now strictly invocation-scoped, so the partial must be
    // untouched relative to a run without the flag.
    let base = PartialAppConfig::empty();

    let with_flag = Query {
        no_title: true,
        ..Default::default()
    }
    .apply_cli_config(None, base.clone(), None)
    .unwrap();
    let without_flag = Query::default().apply_cli_config(None, base, None).unwrap();

    assert_eq!(
        with_flag.conversation.title.generate.auto,
        without_flag.conversation.title.generate.auto,
    );
    assert_eq!(with_flag.conversation.title.generate.auto, None);
}

#[test]
fn picker_new_item_gated_by_new_incompatible_flags() {
    // The picker may only offer "start a new conversation" when `--new` would
    // itself be a legal flag. `--fork` and `--replay` both conflict with
    // `--new` (and still reach the picker), so choosing the item would
    // otherwise silently drop the fork/replay request and manufacture a state
    // clap rejects at parse time.
    assert!(Query::<Settled>::default().allows_new_from_picker());

    let fork: Query = Query {
        fork: Some(None),
        ..Default::default()
    };
    assert!(!fork.allows_new_from_picker());

    let replay: Query = Query {
        replay: true,
        ..Default::default()
    };
    assert!(!replay.allows_new_from_picker());
}

#[test]
fn picker_new_item_gated_by_bare_id_flag() {
    // Bare `jp query --id` parses to an empty value (`FlagIds` sets
    // `default_missing_value = ""`), which becomes the interactive picker.
    // Since `--new` conflicts with the `id` arg, the picker must not offer the
    // synthetic "new" item: choosing it would drop the target and manufacture a
    // `--new --id` state clap rejects at parse time.
    let bare_id: Query = Query {
        target: FlagIds::from_targets(vec![ConversationTarget::Picker(PickerFilter::default())]),
        ..Default::default()
    };
    assert!(!bare_id.allows_new_from_picker());
}

/// The query words a parse produced, for comparison against a static slice.
fn query_words(query: &Query) -> &[String] {
    query.query.as_deref().unwrap_or_default()
}

#[test]
fn bare_quote_uses_the_blockquote_prefix() {
    let query = parse_query(&["--quote"]).unwrap();
    assert_eq!(query.quote, Some(true));
    assert_eq!(query_words(&query), [] as [String; 0]);
}

#[test]
fn quote_true_is_the_same_as_a_bare_quote() {
    let query = parse_query(&["--quote=true"]).unwrap();
    assert_eq!(query.quote, Some(true));
    assert_eq!(query_words(&query), [] as [String; 0]);
}

#[test]
fn quote_false_still_quotes_but_drops_the_prefix() {
    let query = parse_query(&["--quote=false"]).unwrap();
    assert_eq!(query.quote, Some(false));
    assert_eq!(query_words(&query), [] as [String; 0]);
}

#[test]
fn quote_is_absent_when_not_given() {
    let query = parse_query(&[]).unwrap();
    assert_eq!(query.quote, None);
}

#[test]
fn quote_takes_an_unattached_bool() {
    let query = parse_query(&["--quote", "false"]).unwrap();
    assert_eq!(query.quote, Some(false));
    assert_eq!(query_words(&query), [] as [String; 0]);

    let query = parse_query(&["--quote", "true"]).unwrap();
    assert_eq!(query.quote, Some(true));
    assert_eq!(query_words(&query), [] as [String; 0]);
}

#[test]
fn quote_takes_an_unattached_bool_ahead_of_the_query() {
    let query = parse_query(&["--quote", "false", "and", "now?"]).unwrap();
    assert_eq!(query.quote, Some(false));
    assert_eq!(query_words(&query), ["and".to_owned(), "now?".to_owned()]);
}

#[test]
fn quote_does_not_swallow_a_following_flag() {
    let query = parse_query(&["--quote", "--model", "foo"]).unwrap();
    assert_eq!(query.quote, Some(true));
    assert_eq!(query.model.as_deref(), Some("foo"));
}

#[test]
fn quote_does_not_swallow_the_positional_query() {
    let query = parse_query(&["--quote", "what about X?"]).unwrap();
    assert_eq!(query.quote, Some(true));
    assert_eq!(query_words(&query), ["what about X?".to_owned()]);
}

#[test]
fn quote_only_takes_the_word_directly_after_it() {
    // `false` is the query here: it sits before the flag, so it was never
    // offered as the flag's value.
    let query = parse_query(&["false", "--quote"]).unwrap();
    assert_eq!(query.quote, Some(true));
    assert_eq!(query_words(&query), ["false".to_owned()]);

    // Same when another flag sits between the two.
    let query = parse_query(&["--quote", "--model", "foo", "false"]).unwrap();
    assert_eq!(query.quote, Some(true));
    assert_eq!(query_words(&query), ["false".to_owned()]);
}

#[test]
fn a_double_dash_shields_a_bool_from_quote() {
    let query = parse_query(&["--quote", "--", "false"]).unwrap();
    assert_eq!(query.quote, Some(true));
    assert_eq!(query_words(&query), ["false".to_owned()]);
}

#[test]
fn words_before_and_after_a_double_dash_form_one_query() {
    let query = parse_query(&["why", "--", "--model", "foo"]).unwrap();
    assert_eq!(query.quote, None);
    assert_eq!(query_words(&query), [
        "why".to_owned(),
        "--model".to_owned(),
        "foo".to_owned()
    ]);
}

#[test]
fn quote_with_an_attached_value_leaves_a_following_bool_in_the_query() {
    // `--quote=false` has its value already, so the `true` after it is query
    // text. Without the attached/bare distinction both forms would look
    // identical here, since clap records the same index for either.
    let query = parse_query(&["--quote=false", "true"]).unwrap();
    assert_eq!(query.quote, Some(false));
    assert_eq!(query_words(&query), ["true".to_owned()]);
}

#[test]
fn quote_rejects_an_attached_non_boolean_value() {
    assert!(parse_query(&["--quote=foo"]).is_err());
}

/// Read `jp query <args>` through the real command line, leaving undecided what
/// needs the configuration.
fn parsed_query(args: &[&str]) -> std::result::Result<Query<Parsed>, clap::Error> {
    let argv = ["jp", "query"].into_iter().chain(args.iter().copied());
    let Commands::Query(query) = Cli::try_parse_from(argv)?.command else {
        panic!("`jp query` parses as a query");
    };

    Ok(query)
}

/// Read `jp query <args>` and settle it with no tools configured and nobody to
/// ask, which only `-t` naming an unknown tool before the query could refuse.
fn query_from(args: &[&str]) -> std::result::Result<Query, clap::Error> {
    Ok(parsed_query(args)?
        .settle(&PartialAppConfig::default(), &ArgPrompts::default())
        .expect("settles without asking"))
}

/// [`query_from`], always with `--no-edit` so no editor opens.
fn parse_query(args: &[&str]) -> std::result::Result<Query, clap::Error> {
    let argv: Vec<&str> = ["--no-edit"]
        .into_iter()
        .chain(args.iter().copied())
        .collect();
    query_from(&argv)
}

// A query naming an unreadable file must fail before any conversation or
// session state is touched. The read used to happen after the conversation was
// created and recorded as the session's active one, so a typo'd path left an
// empty conversation behind that the next query silently targeted.
#[test]
fn run_missing_at_path_query_leaves_conversation_and_session_untouched() {
    let dir = camino_tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.md");

    let session = Session {
        id: SessionId::new("jp-cli-query-test").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };
    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    let workspace = Workspace::in_memory("/tmp/jp-cli-query-test");
    let mut ctx = Ctx::new(
        crate::bootstrap::ExecutionContext::for_workspace(&workspace),
        workspace,
        None,
        Runtime::new().unwrap(),
        Globals::default(),
        AppConfig::new_test(),
        Some(session.clone()),
        printer,
    );

    let query = parse_query(&["--new", &format!("@{missing}")]).unwrap();
    let result = Runtime::new()
        .unwrap()
        .block_on(query.run(&mut ctx, None, false));

    let Err(error) = result else {
        panic!("a query naming a missing file must fail");
    };
    // Pin that it failed on the unreadable path, not on something the test
    // environment happens to be missing further down `run`.
    assert_eq!(
        error
            .metadata
            .iter()
            .find(|(key, _)| key == "path")
            .map(|(_, value)| value),
        Some(&Value::from(missing.as_str())),
    );

    assert_eq!(ctx.workspace.conversations().count(), 0);
    assert_eq!(ctx.workspace.session_active_conversation(&session), None);
}

/// A context whose editor leaves the seeded draft exactly as JP wrote it, which
/// is what quitting without typing looks like to the query parser.
///
/// The temp dir comes back so the caller keeps it alive.
fn empty_editor_ctx(session: &Session) -> (Ctx, SharedBuffer, SharedBuffer, Utf8TempDir) {
    editor_ctx(session, "true")
}

/// A context that runs `editor_cmd` as the user's editor.
///
/// The temp dir comes back so the caller keeps it alive.
fn editor_ctx(
    session: &Session,
    editor_cmd: &str,
) -> (Ctx, SharedBuffer, SharedBuffer, Utf8TempDir) {
    let tmp = camino_tempfile::tempdir().unwrap();
    let root = tmp.path();
    // User-local storage keeps the editor's draft out of the workspace tree,
    // which is where the real CLI puts it.
    let fs = Arc::new(
        FsStorageBackend::new(&root.join(".jp"))
            .unwrap()
            .with_user_storage(&root.join("user"), None, "abc")
            .unwrap(),
    );

    let (printer, out, err) = Printer::memory(OutputFormat::TextPretty);
    let workspace = Workspace::in_memory(root).with_backend(Arc::clone(&fs));

    let mut config = config_with_model(ProviderId::Test, "mock");
    config.editor.cmd = Some(CommandConfigOrString::String(editor_cmd.to_owned()));
    // The test provider streams nothing, so a turn that starts fails. Retrying
    // it five times with backoff is time these tests would only spend waiting.
    config.assistant.request.max_retries = 0;

    let mut ctx = Ctx::new(
        crate::bootstrap::ExecutionContext::for_workspace(&workspace),
        workspace,
        Some(fs),
        Runtime::new().unwrap(),
        Globals::default(),
        config,
        Some(session.clone()),
        printer,
    );

    // Opening the editor is the path these tests exercise, and the editor only
    // opens for a user who can close it. `Ctx::new` reads that off the
    // process's own stdout, which the in-memory printer does not replace, so
    // pin it rather than inherit whatever the test binary was launched with.
    ctx.term.interactive = true;

    (ctx, out, err, tmp)
}

/// A backend over the same two roots [`editor_ctx`] configures.
fn test_backend(root: &Utf8Path) -> FsStorageBackend {
    FsStorageBackend::new(&root.join(".jp"))
        .unwrap()
        .with_user_storage(&root.join("user"), None, "abc")
        .unwrap()
}

/// Every conversation the backend can see, across both storage roots.
///
/// [`projected_conversation_ids`] scans the workspace root alone, so it cannot
/// see the durable user-local store the editor's draft directory lives in.
fn stored_conversation_ids(root: &Utf8Path) -> Vec<ConversationId> {
    test_backend(root).load_conversation_ids(ConversationFilter::default())
}

/// Assert that every conversation directory in the store passes validation.
fn assert_store_holds_only_conversations(root: &Utf8Path) {
    let result = test_backend(root).validate_conversations();

    assert!(result.invalid.is_empty(), "invalid: {:?}", result.invalid);
}

/// Run `jp query <args>` to completion, without `parse_query`'s `--no-edit`:
/// opening the editor is the path these tests exercise.
fn run_query(ctx: &mut Ctx, args: &[&str]) -> crate::cmd::Output {
    let query = query_from(args).unwrap();

    let result = Runtime::new()
        .unwrap()
        .block_on(query.run(ctx, None, false));
    ctx.printer.flush();
    result
}

// Quitting the editor without typing anything leaves no trace: the `--new`
// conversation is never written to storage, and the session is left pointing
// wherever it pointed before. Recording the activation is what writes it: the
// `last_activated_at` bump is the first mutation a fresh conversation gets, so
// this pins that no activation is recorded for a query that was ignored.
#[test]
fn run_empty_editor_query_stores_no_conversation() {
    let session = Session {
        id: SessionId::new("jp-cli-empty-query-test").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };
    let (mut ctx, out, err, tmp) = empty_editor_ctx(&session);

    run_query(&mut ctx, &["--new"]).unwrap();

    assert_eq!(out.lock().as_str(), "Query is empty, ignoring.\n");
    assert_eq!(err.lock().as_str(), "");
    assert_eq!(stored_conversation_ids(tmp.path()), []);
    assert_store_holds_only_conversations(tmp.path());
    assert_eq!(ctx.workspace.session_active_conversation(&session), None);
}

// `--title` names the title the run ends with, and the draft's directory is
// named after it, so it is resolved before the request is composed. Storing it
// is what must wait: an abandoned query leaves no titled conversation behind.
#[test]
fn run_empty_titled_query_stores_no_conversation() {
    let session = Session {
        id: SessionId::new("jp-cli-empty-title-test").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };
    let (mut ctx, out, _err, tmp) = empty_editor_ctx(&session);

    run_query(&mut ctx, &["--new", "--title", "a title"]).unwrap();

    assert_eq!(out.lock().as_str(), "Query is empty, ignoring.\n");
    assert_eq!(stored_conversation_ids(tmp.path()), []);
    assert_store_holds_only_conversations(tmp.path());
}

// A bare `--fork` keeps every turn, and a value keeps that many trailing turns.
//
// The reported failure: the flag declared a `default_missing_value` fed through
// a parser returning `Option<usize>`, so clap registered the value as one type
// and the derived reader asked for another. Every `jp query --fork` panicked
// while parsing its own arguments.
#[test]
fn fork_flag_parses_with_and_without_a_turn_count() {
    let bare = query_from(&["--fork"]).unwrap();
    assert_eq!(bare.fork, Some(None));

    let counted = query_from(&["--fork=2"]).unwrap();
    assert_eq!(counted.fork, Some(Some(2)));
}

#[test]
fn fork_flag_rejects_a_non_numeric_turn_count() {
    let Err(error) = query_from(&["--fork=x"]) else {
        panic!("a non-numeric turn count must be rejected");
    };

    assert!(
        error.to_string().contains("expected a positive integer"),
        "unexpected error: {error}"
    );
}

/// Seed a written conversation holding one completed turn, and return a handle
/// to it.
fn seed_conversation(ctx: &mut Ctx) -> ConversationHandle {
    let id = make_id(1_700_000_000);
    ctx.workspace.create_conversation_with_id(
        id,
        Conversation::default().with_last_activated_at(ctx.now()),
        ctx.config(),
    );

    let handle = ctx.workspace.acquire_conversation(&id).unwrap();
    let lock = ctx.workspace.test_lock(handle);
    lock.as_mut().update_events(|events| {
        events.start_turn(ChatRequest::from("seeded question"));
        events.extend([ConversationEvent::now(ChatResponse::message(
            "seeded answer",
        ))]);
    });
    drop(lock);

    ctx.workspace.acquire_conversation(&id).unwrap()
}

// `--expires-in` used to be stamped and flushed while the conversation was
// being created, which wrote it before there was any request to send.
#[test]
fn run_empty_expiring_query_stores_no_conversation() {
    let session = Session {
        id: SessionId::new("jp-cli-empty-expiry-test").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };
    let (mut ctx, out, _err, tmp) = empty_editor_ctx(&session);

    run_query(&mut ctx, &["--new", "--tmp=1h"]).unwrap();

    assert_eq!(out.lock().as_str(), "Query is empty, ignoring.\n");
    assert_eq!(stored_conversation_ids(tmp.path()), []);
    assert_store_holds_only_conversations(tmp.path());
}

// A `--fork` query builds the fork before the request is composed, because the
// editor's history preview and `--replay` both read the fork's inherited
// stream. Abandoning the query must leave the source alone on disk.
#[test]
fn run_empty_forking_query_leaves_only_the_source() {
    let session = Session {
        id: SessionId::new("jp-cli-empty-fork-test").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };
    let (mut ctx, out, _err, tmp) = empty_editor_ctx(&session);
    let source = seed_conversation(&mut ctx);
    let source_id = source.id();

    let query = query_from(&["--fork"]).unwrap();
    Runtime::new()
        .unwrap()
        .block_on(query.run(&mut ctx, Some(source), false))
        .unwrap();
    ctx.printer.flush();

    assert_eq!(out.lock().as_str(), "Query is empty, ignoring.\n");
    assert_eq!(stored_conversation_ids(tmp.path()), [source_id]);
    assert_store_holds_only_conversations(tmp.path());
}

// `--compact` is staged against the stream the request is composed against, so
// the editor's history preview shows the compacted conversation. Abandoning the
// query must leave the stored stream as it was.
#[test]
fn run_empty_compacting_query_stores_no_compaction() {
    let session = Session {
        id: SessionId::new("jp-cli-empty-compact-test").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };
    let (mut ctx, out, _err, tmp) = empty_editor_ctx(&session);
    let source = seed_conversation(&mut ctx);
    let source_id = source.id();

    let query = query_from(&["--compact=r:.."]).unwrap();
    Runtime::new()
        .unwrap()
        .block_on(query.run(&mut ctx, Some(source), false))
        .unwrap();
    ctx.printer.flush();

    assert_eq!(out.lock().as_str(), "Query is empty, ignoring.\n");

    // The compaction was staged, so the run really did exercise the path: what
    // the assertion below pins is that staging it never reached storage.
    let handle = ctx.workspace.acquire_conversation(&source_id).unwrap();
    assert_eq!(
        ctx.workspace.events(&handle).unwrap().compactions().count(),
        1,
        "the compaction must have been staged in memory"
    );

    let storage = FsStorageBackend::new(&tmp.path().join(".jp")).unwrap();
    let stored = storage
        .load_conversation_stream(&source_id, &PartialAppConfig::empty())
        .unwrap();
    assert_eq!(stored.compactions().count(), 0);

    assert_store_holds_only_conversations(tmp.path());
}

// Not only the empty query: any failure before the turn starts leaves the
// staged conversation unwritten. Here the editor cannot be spawned, which
// aborts the run after `--tmp` and the fresh conversation are already staged.
#[test]
fn run_failing_before_the_turn_stores_no_conversation() {
    let session = Session {
        id: SessionId::new("jp-cli-failed-setup-test").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };
    let (mut ctx, _out, _err, tmp) = editor_ctx(&session, "jp-no-such-editor-binary");

    let Err(error) = run_query(&mut ctx, &["--new", "--tmp=1h"]) else {
        panic!("a query whose editor cannot be spawned must fail");
    };
    assert_eq!(error.message.as_deref(), Some("Editor error"));

    assert_eq!(stored_conversation_ids(tmp.path()), []);
    assert_store_holds_only_conversations(tmp.path());
}

// Under `--no-persist`, a failed inline query does not write a recovery draft
// or create a user-local conversation directory.
#[test]
fn run_failing_no_persist_query_leaves_no_conversation_directory() {
    let session = Session {
        id: SessionId::new("jp-cli-no-persist-draft-test").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };
    let tmp = camino_tempfile::tempdir().unwrap();
    let root = tmp.path();
    let fs = Arc::new(test_backend(root));
    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);

    // The workspace as `load_workspace` opens it for `--no-persist`.
    let workspace = Workspace::in_memory(root)
        .with_backend(Arc::clone(&fs))
        .without_persistence();

    let mut config = config_with_model(ProviderId::Test, "mock");
    config.assistant.request.max_retries = 0;

    let mut ctx = Ctx::new(
        ExecutionContext::for_workspace(&workspace),
        workspace,
        Some(fs),
        Runtime::new().unwrap(),
        Globals::default(),
        config,
        Some(session),
        printer,
    );

    let error = run_query(&mut ctx, &["--new", "Reply with: ok"]).unwrap_err();
    assert_eq!(error.message.as_deref(), Some("Stream error"));

    let user_conversations = test_backend(root)
        .user_storage_path()
        .unwrap()
        .join("conversations");
    let dirs: Vec<String> = std::fs::read_dir(&user_conversations).map_or_default(|entries| {
        entries
            .map(|entry| entry.unwrap())
            .filter(|entry| entry.path().is_dir())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect()
    });
    assert_eq!(dirs, Vec::<String>::new());

    assert_store_holds_only_conversations(root);
}

// A run that renames the conversation composes its draft in the directory the
// conversation lives in now, not the one its new title will name. Naming the
// draft's directory after the uncommitted title put it beside the real one,
// where the next write deleted it as a stale copy of the same id — taking the
// request the user had just typed.
#[test]
fn run_renaming_query_keeps_the_composed_draft() {
    let session = Session {
        id: SessionId::new("jp-cli-rename-draft-test").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };
    // An editor that types for the user, so the request is non-empty and the
    // draft it leaves behind is the only copy of what was composed.
    let (mut ctx, _out, _err, tmp) = editor_ctx(
        &session,
        r#"sh -c 'printf "typed request\n" > "$1"' jp-editor"#,
    );

    let id = make_id(1_700_000_000);
    ctx.workspace.create_conversation_with_id(
        id,
        Conversation {
            title: Some("Old".to_owned()),
            ..Conversation::default().with_last_activated_at(ctx.now())
        },
        ctx.config(),
    );
    let handle = ctx.workspace.acquire_conversation(&id).unwrap();
    let lock = ctx.workspace.test_lock(handle);
    lock.as_mut()
        .update_events(|events| events.start_turn(ChatRequest::from("seeded question")));
    drop(lock);

    let source = ctx.workspace.acquire_conversation(&id).unwrap();
    let query = query_from(&["--edit", "--title", "New"]).unwrap();
    let Err(error) = Runtime::new()
        .unwrap()
        .block_on(query.run(&mut ctx, Some(source), false))
    else {
        panic!("the test provider streams nothing, so the turn must fail");
    };
    assert_eq!(error.message.as_deref(), Some("Stream error"));

    let dir = ctx
        .fs_backend
        .as_deref()
        .unwrap()
        .find_user_local_conversation_dir(&id)
        .expect("the conversation still has a user-local directory");
    let draft = std::fs::read_to_string(dir.join(editor::QUERY_FILENAME))
        .expect("the composed request is still recoverable");
    assert_eq!(draft, "typed request\n");

    // The rename did happen, so the draft survived it rather than the title
    // having been quietly dropped.
    let storage = FsStorageBackend::new(&tmp.path().join(".jp")).unwrap();
    let metadata = storage.load_conversation_metadata(&id).unwrap();
    assert_eq!(metadata.title.as_deref(), Some("New"));
}

#[test]
fn run_new_query_stores_the_conversation_before_the_draft() {
    let session = Session {
        id: SessionId::new("jp-cli-draft-order-test").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };
    let (mut ctx, _out, _err, tmp) = editor_ctx(
        &session,
        r#"sh -c 'LC_ALL=C ls "$(dirname "$1")" > "$1"' jp-editor"#,
    );

    let error = run_query(&mut ctx, &["--new"]).unwrap_err();
    assert_eq!(error.message.as_deref(), Some("Stream error"));

    let [id] = <[ConversationId; 1]>::try_from(stored_conversation_ids(tmp.path())).unwrap();
    let dir = test_backend(tmp.path())
        .find_user_local_conversation_dir(&id)
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join(editor::QUERY_FILENAME)).unwrap(),
        "QUERY_MESSAGE.md\nbase_config.json\nevents.json\nmetadata.json\n"
    );
    assert_store_holds_only_conversations(tmp.path());
}

#[test]
fn run_no_persist_editor_query_writes_no_draft() {
    let session = Session {
        id: SessionId::new("jp-cli-no-persist-editor-test").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };
    let (ctx, _out, _err, tmp) = editor_ctx(
        &session,
        r#"sh -c 'printf "typed request\n" > "$1"' jp-editor"#,
    );
    let mut ctx = no_persist(ctx);

    let error = run_query(&mut ctx, &["--new"]).unwrap_err();
    assert_eq!(error.message.as_deref(), Some("Stream error"));

    let backend = test_backend(tmp.path());
    let user = backend.user_storage_path().unwrap();
    assert!(!user.join("conversations").exists());
    assert_eq!(stored_conversation_ids(tmp.path()), []);
}

fn conversation_with_saved_draft(ctx: &mut Ctx, draft: &str) -> (ConversationId, Utf8PathBuf) {
    let id = make_id(1_700_000_000);
    ctx.workspace.create_conversation_with_id(
        id,
        Conversation::default().with_last_activated_at(ctx.now()),
        ctx.config(),
    );
    let handle = ctx.workspace.acquire_conversation(&id).unwrap();
    let lock = ctx.workspace.test_lock(handle);
    lock.as_mut()
        .update_events(|events| events.start_turn(ChatRequest::from("seeded question")));
    drop(lock);

    let path = ctx
        .fs_backend
        .as_deref()
        .unwrap()
        .find_user_local_conversation_dir(&id)
        .expect("the stored conversation has a user-local directory")
        .join(editor::QUERY_FILENAME);
    std::fs::write(&path, draft).unwrap();

    (id, path)
}

#[test]
fn run_no_persist_empty_editor_query_keeps_the_saved_draft() {
    let session = Session {
        id: SessionId::new("jp-cli-no-persist-empty-draft-test").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };
    let (mut ctx, out, _err, _tmp) = editor_ctx(&session, r#"sh -c ': > "$1"' jp-editor"#);
    let (id, draft) = conversation_with_saved_draft(&mut ctx, "saved request\n");
    let mut ctx = no_persist(ctx);

    let source = ctx.workspace.acquire_conversation(&id).unwrap();
    let query = query_from(&["--edit"]).unwrap();
    Runtime::new()
        .unwrap()
        .block_on(query.run(&mut ctx, Some(source), false))
        .unwrap();
    ctx.printer.flush();

    assert_eq!(out.lock().as_str(), "Query is empty, ignoring.\n");
    assert_eq!(std::fs::read_to_string(&draft).unwrap(), "saved request\n");
}

#[test]
fn run_no_persist_editor_query_starts_from_the_saved_draft() {
    let session = Session {
        id: SessionId::new("jp-cli-no-persist-seeded-draft-test").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };
    let seen_dir = camino_tempfile::tempdir().unwrap();
    let seen = seen_dir.path().join("seen");
    // Shell-quote the path so `shlex::split` preserves backslashes on Windows.
    let seen_arg = shlex::try_quote(seen.as_str()).unwrap();
    let (mut ctx, _out, _err, _tmp) =
        editor_ctx(&session, &format!(r#"sh -c 'cp "$1" "$0"' {seen_arg}"#));
    let (id, draft) = conversation_with_saved_draft(&mut ctx, "saved request\n");
    let mut ctx = no_persist(ctx);

    let source = ctx.workspace.acquire_conversation(&id).unwrap();
    let query = query_from(&["--edit"]).unwrap();
    let error = Runtime::new()
        .unwrap()
        .block_on(query.run(&mut ctx, Some(source), false))
        .unwrap_err();
    assert_eq!(error.message.as_deref(), Some("Stream error"));

    // The draft file also holds config-dependent sections after the request,
    // so only its start is checked.
    let shown = std::fs::read_to_string(&seen).unwrap();
    assert!(shown.starts_with("saved request\n"), "{shown}");
    assert_eq!(std::fs::read_to_string(&draft).unwrap(), "saved request\n");
}

fn no_persist(mut ctx: Ctx) -> Ctx {
    let workspace = std::mem::replace(&mut ctx.workspace, Workspace::in_memory("/unused"));
    ctx.workspace = workspace.without_persistence();
    ctx
}

// The other side of the deferral for `--tmp`: a query that does go ahead stores
// the expiry, stamped from the conversation's own creation time.
#[test]
fn run_expiring_query_stores_the_expiry_before_the_turn() {
    let session = Session {
        id: SessionId::new("jp-cli-stored-expiry-test").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };
    let (mut ctx, _out, _err, tmp) = empty_editor_ctx(&session);

    // An inline query skips the editor, so the request is non-empty.
    let error = run_query(&mut ctx, &["--new", "--tmp=1h", "hello"]).unwrap_err();
    assert_eq!(error.message.as_deref(), Some("Stream error"));

    let storage_dir = tmp.path().join(".jp");
    let ids = projected_conversation_ids(&storage_dir);
    assert_eq!(ids.len(), 1, "expected one stored conversation: {ids:?}");

    let storage = FsStorageBackend::new(&storage_dir).unwrap();
    let metadata = storage.load_conversation_metadata(&ids[0]).unwrap();
    assert_eq!(
        metadata.expires_at,
        Some(ids[0].timestamp() + chrono::Duration::hours(1))
    );
}

// The other side of the deferral for `--fork`: a query that does go ahead
// writes the fork, with the source's turns inherited.
#[test]
fn run_forking_query_stores_the_fork_before_the_turn() {
    let session = Session {
        id: SessionId::new("jp-cli-stored-fork-test").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };
    let (mut ctx, _out, _err, tmp) = empty_editor_ctx(&session);
    let source = seed_conversation(&mut ctx);
    let source_id = source.id();

    // An inline query skips the editor, so the request is non-empty. It goes
    // ahead of `--fork`, which would otherwise swallow it as its turn count.
    let query = query_from(&["hello", "--fork"]).unwrap();
    let Err(error) = Runtime::new()
        .unwrap()
        .block_on(query.run(&mut ctx, Some(source), false))
    else {
        panic!("the test provider streams nothing, so the turn must fail");
    };
    assert_eq!(error.message.as_deref(), Some("Stream error"));

    let storage_dir = tmp.path().join(".jp");
    let ids = projected_conversation_ids(&storage_dir);
    assert_eq!(ids.len(), 2, "source and fork are both on disk: {ids:?}");

    let fork_id = ids
        .into_iter()
        .find(|id| *id != source_id)
        .expect("the fork is one of the two");
    let storage = FsStorageBackend::new(&storage_dir).unwrap();
    let stored = storage
        .load_conversation_stream(&fork_id, &PartialAppConfig::empty())
        .unwrap();

    assert_eq!(
        stored.turn_count(),
        2,
        "the fork inherited the source's turn and started its own"
    );
}

// The other side of the deferral: a query that does go ahead ends with the
// title `--title` named, written to storage before the turn starts. The turn
// then fails here, because the test provider streams nothing.
#[test]
fn run_titled_query_stores_the_title_before_the_turn() {
    let session = Session {
        id: SessionId::new("jp-cli-stored-title-test").unwrap(),
        source: SessionSource::env("JP_SESSION"),
    };
    let (mut ctx, _out, _err, tmp) = empty_editor_ctx(&session);

    // An inline query skips the editor, so the request is non-empty.
    let error = run_query(&mut ctx, &["--new", "--title", "a title", "hello"]).unwrap_err();
    assert_eq!(error.message.as_deref(), Some("Stream error"));

    let storage_dir = tmp.path().join(".jp");
    let ids = projected_conversation_ids(&storage_dir);
    assert_eq!(ids.len(), 1, "expected one stored conversation: {ids:?}");

    let storage = FsStorageBackend::new(&storage_dir).unwrap();
    let metadata = storage.load_conversation_metadata(&ids[0]).unwrap();
    assert_eq!(metadata.title.as_deref(), Some("a title"));

    assert_eq!(
        ctx.workspace.session_active_conversation(&session),
        Some(ids[0])
    );
}

#[test]
fn resolve_query_missing_at_path_errors() {
    let dir = camino_tempfile::tempdir().unwrap();
    let path = dir.path().join("missing.md");
    let query = parse_query(&[&format!("@{path}")]).unwrap();

    let Err(error) = query.resolve_query() else {
        panic!("a query naming a missing file must fail, not be sent verbatim");
    };
    assert_matches!(&error, Error::ArgFile { path: p, .. } if p == path.as_str());
}

/// Read `jp query <args>` and settle it against `partial`, asking `prompts`
/// about a `-t` value that names no tool.
fn try_settled_query(
    args: &[&str],
    partial: &PartialAppConfig,
    prompts: &ArgPrompts,
) -> std::result::Result<Query, SettleError> {
    parsed_query(args).unwrap().settle(partial, prompts)
}

/// [`try_settled_query`] with nobody to ask.
fn settled_query(args: &[&str], partial: &PartialAppConfig) -> Query {
    try_settled_query(args, partial, &ArgPrompts::default()).unwrap()
}

/// [`try_settled_query`] with a user who confirms once.
fn confirmed_query(args: &[&str], partial: &PartialAppConfig) -> Query {
    try_settled_query(args, partial, &answering(['y'])).unwrap()
}

/// Prompts answered with `answers` in order; a prompt past the last answer is
/// dismissed with Esc.
fn answering(answers: impl IntoIterator<Item = char>) -> ArgPrompts {
    ArgPrompts::new(Arc::new(
        MockPromptBackend::new().with_inline_responses(answers),
    ))
}

/// The directives `jp query <args>` applies when no tools are configured.
fn parsed_directives(args: &[&str]) -> Vec<ToolDirective> {
    settled_query(args, &PartialAppConfig::default()).tool_directives
}

/// `words` as query words.
#[expect(
    clippy::unnecessary_wraps,
    reason = "compared against the `Option` a query holds"
)]
fn words(words: &[&str]) -> Option<Vec<String>> {
    Some(words.iter().map(ToString::to_string).collect())
}

#[test]
fn tool_flag_splits_comma_separated_names() {
    assert_eq!(parsed_directives(&["-tfoo,bar,baz"]), [
        ToolDirective::Enable("foo".into()),
        ToolDirective::Enable("bar".into()),
        ToolDirective::Enable("baz".into()),
    ]);
}

#[test]
fn no_tool_flag_splits_comma_separated_names() {
    assert_eq!(parsed_directives(&["--no-tool=foo,bar"]), [
        ToolDirective::Disable("foo".into()),
        ToolDirective::Disable("bar".into()),
    ]);
}

// Splitting happens before the directives are ordered, so the names of one
// flag stay together and in the order they were written.
#[test]
fn comma_separated_names_keep_their_place_in_the_directive_order() {
    assert_eq!(parsed_directives(&["-Tfoo,bar", "-tbaz", "-Tqux"]), [
        ToolDirective::Disable("foo".into()),
        ToolDirective::Disable("bar".into()),
        ToolDirective::Enable("baz".into()),
        ToolDirective::Disable("qux".into()),
    ]);
}

#[test]
fn tool_flag_trims_space_around_comma_separated_names() {
    assert_eq!(parsed_directives(&["--tool", "foo, bar"]), [
        ToolDirective::Enable("foo".into()),
        ToolDirective::Enable("bar".into()),
    ]);
}

// A bare `-t` still means "every tool", and must not be read as one empty name.
#[test]
fn bare_tool_flag_stays_a_bulk_directive() {
    assert_eq!(parsed_directives(&["-t", "-T"]), [
        ToolDirective::EnableAll,
        ToolDirective::DisableAll,
    ]);
}

#[test]
fn a_confirmed_word_after_tool_flag_naming_no_tool_starts_the_query() {
    let query = confirmed_query(&["-t", "try", "again"], &make_partial_with_tools());

    assert_eq!(query.tool_directives, [ToolDirective::EnableAll]);
    assert_eq!(query.query, words(&["try", "again"]));
}

// Reading the word as query text enables every tool, durably, so a typo in a
// tool name must not get there unasked.
#[test]
fn a_word_after_tool_flag_naming_no_tool_is_refused_without_a_user() {
    let Err(error) = try_settled_query(
        &["-t", "fs_raed_file", "explain"],
        &make_partial_with_tools(),
        &ArgPrompts::default(),
    ) else {
        panic!("an unconfirmed word must not enable every tool");
    };

    assert_eq!(
        error.to_string(),
        "`fs_raed_file` is not a configured tool. To enable every tool and start the query with \
         `fs_raed_file`, put the query after `--`: `-t -- fs_raed_file ...`"
    );
}

#[test]
fn a_declined_word_after_tool_flag_is_refused() {
    let Err(error) = try_settled_query(
        &["-t", "etote", "it"],
        &make_partial_with_tools(),
        &answering(['n']),
    ) else {
        panic!("a declined word must not enable every tool");
    };

    assert_eq!(
        error.to_string(),
        "`etote` is not a configured tool. To enable every tool and start the query with `etote`, \
         put the query after `--`: `-t -- etote ...`"
    );
}

// Esc backs out of the question, which must count as a no.
#[test]
fn a_dismissed_question_refuses_the_word() {
    let Err(error) = try_settled_query(
        &["-t", "try", "again"],
        &make_partial_with_tools(),
        &answering([]),
    ) else {
        panic!("Esc must not enable every tool");
    };

    assert_eq!(
        error.to_string(),
        "`try` is not a configured tool. To enable every tool and start the query with `try`, put \
         the query after `--`: `-t -- try ...`"
    );
}

// `=` says the value is the flag's, so it names tools however the query after
// it reads: an unknown name is an error, not a question.
#[test]
fn an_attached_value_before_the_query_always_names_tools() {
    let query = settled_query(&["-t=explicit_tool", "go"], &make_partial_with_tools());
    assert_eq!(query.tool_directives, [ToolDirective::Enable(
        "explicit_tool".into()
    )]);
    assert_eq!(query.query, words(&["go"]));

    // Named as a tool, the typo reaches the same check a lone `-t typo` does.
    let query = settled_query(&["-t=try", "again"], &make_partial_with_tools());
    assert_eq!(query.tool_directives, [ToolDirective::Enable("try".into())]);
    assert_eq!(query.query, words(&["again"]));

    let query = settled_query(&["-ttry", "again"], &make_partial_with_tools());
    assert_eq!(query.tool_directives, [ToolDirective::Enable("try".into())]);
}

// Disabling every tool grants nothing, so nobody is asked.
#[test]
fn a_word_after_no_tool_flag_naming_no_tool_starts_the_query() {
    let query = settled_query(&["-T", "what", "now?"], &make_partial_with_tools());

    assert_eq!(query.tool_directives, [ToolDirective::DisableAll]);
    assert_eq!(query.query, words(&["what", "now?"]));
}

// Nobody to ask, so the word settling at all shows nobody was asked.
#[test]
fn a_word_after_tool_flag_naming_a_tool_stays_a_tool_name() {
    let query = settled_query(&["-t", "explicit_tool", "go"], &make_partial_with_tools());

    assert_eq!(query.tool_directives, [ToolDirective::Enable(
        "explicit_tool".into()
    )]);
    assert_eq!(query.query, words(&["go"]));
}

// A built-in's name is a tool name even when no config declares it.
#[test]
fn a_word_naming_a_builtin_tool_stays_a_tool_name() {
    let query = settled_query(
        &["-t", "describe_tools", "go"],
        &PartialAppConfig::default(),
    );

    assert_eq!(query.tool_directives, [ToolDirective::Enable(
        "describe_tools".into()
    )]);
    assert_eq!(query.query, words(&["go"]));
}

#[test]
fn a_word_listing_only_known_tools_stays_a_tool_list() {
    let query = settled_query(
        &["-t", "explicit_tool,implicitly_enabled_tool", "go"],
        &make_partial_with_tools(),
    );

    assert_eq!(query.tool_directives, [
        ToolDirective::Enable("explicit_tool".into()),
        ToolDirective::Enable("implicitly_enabled_tool".into()),
    ]);
    assert_eq!(query.query, words(&["go"]));
}

// One unknown name makes the whole list a candidate for query text, which
// needs the same confirmation; the known name must not be enabled meanwhile.
#[test]
fn a_word_listing_an_unknown_tool_is_refused_without_a_user() {
    let Err(error) = try_settled_query(
        &["-t", "explicit_tool,nope", "go"],
        &make_partial_with_tools(),
        &ArgPrompts::default(),
    ) else {
        panic!("a list naming an unknown tool must not settle unasked");
    };

    assert_eq!(
        error.to_string(),
        "`explicit_tool,nope` is not a configured tool. To enable every tool and start the query \
         with `explicit_tool,nope`, put the query after `--`: `-t -- explicit_tool,nope ...`"
    );
}

#[test]
fn a_confirmed_word_listing_an_unknown_tool_starts_the_query() {
    let query = confirmed_query(
        &["-t", "explicit_tool,nope", "go"],
        &make_partial_with_tools(),
    );

    assert_eq!(query.tool_directives, [ToolDirective::EnableAll]);
    assert_eq!(query.query, words(&["explicit_tool,nope", "go"]));
}

// `hello,` is not a valid tool list, which must not fail the parse before the
// word gets the chance to be query text.
#[test]
fn a_word_ending_in_a_comma_starts_the_query() {
    let query = confirmed_query(&["-t", "hello,", "world"], &make_partial_with_tools());

    assert_eq!(query.tool_directives, [ToolDirective::EnableAll]);
    assert_eq!(query.query, words(&["hello,", "world"]));
}

#[test]
fn a_word_given_back_to_the_query_keeps_its_place() {
    let query = confirmed_query(
        &["please", "-t", "try", "again", "--", "later"],
        &make_partial_with_tools(),
    );

    assert_eq!(query.query, words(&["please", "try", "again", "later"]));
}

// `--quote` takes a word out of the query first, which must not shift where the
// tool word goes back in.
#[test]
fn a_word_given_back_to_the_query_keeps_its_place_around_quote() {
    let before = confirmed_query(
        &["--quote", "false", "-t", "try", "again"],
        &make_partial_with_tools(),
    );
    assert_eq!(before.quote, Some(false));
    assert_eq!(before.query, words(&["try", "again"]));

    let after = confirmed_query(
        &["-t", "try", "again", "--quote", "false"],
        &make_partial_with_tools(),
    );
    assert_eq!(after.quote, Some(false));
    assert_eq!(after.query, words(&["try", "again"]));
}

// Without query text after it, the word can only be meant as a tool name, so a
// typo is reported instead of being sent to the assistant.
#[test]
fn a_lone_word_after_tool_flag_stays_a_tool_name() {
    let query = settled_query(&["-t", "typo"], &make_partial_with_tools());

    assert_eq!(query.tool_directives, [ToolDirective::Enable(
        "typo".into()
    )]);
    assert_eq!(query.query, None);
}

#[test]
fn a_word_followed_by_a_flag_stays_a_tool_name() {
    let query = settled_query(
        &["-t", "typo", "--hide-reasoning", "hello"],
        &make_partial_with_tools(),
    );

    assert_eq!(query.tool_directives, [ToolDirective::Enable(
        "typo".into()
    )]);
    assert_eq!(query.query, words(&["hello"]));
}

// The settled directive is what reaches the configuration.
#[test]
fn a_confirmed_tool_word_enables_every_tool() {
    let query = confirmed_query(&["-t", "try", "again"], &make_partial_with_tools());

    let partial =
        IntoPartialAppConfig::apply_cli_config(&query, None, make_partial_with_tools(), None)
            .unwrap();

    // Off in the base config, so only the bulk directive turns it on.
    assert!(effective(&partial, "explicitly_disabled_tool").state);
}

// Without a user there is no prompt to answer, so the run is refused rather
// than left to guess.
#[test]
fn resolve_config_refuses_an_unknown_tool_word_without_a_user() {
    let tmp = Utf8TempDir::new().unwrap();
    let mut workspace = Workspace::in_memory(tmp.path());
    workspace.load_conversation_index();
    let base = Arc::new(AppConfig::new_test());
    let conversation_id = make_id(4002);
    workspace.create_conversation_with_id(
        conversation_id,
        Conversation::default(),
        Arc::clone(&base),
    );
    let id = conversation_id.to_string();
    let cli = Cli::try_parse_from(["jp", "query", "--id", &id, "-t", "try", "again"]).unwrap();

    let error = resolve_config(
        cli.command,
        || Ok(base.to_partial()),
        &cli.globals.config,
        &mut workspace,
        None,
        None,
        false,
    )
    .unwrap_err();

    assert_matches!(
        error,
        crate::error::Error::CliConfig(message) if message ==
            "`try` is not a configured tool. To enable every tool and start the query with \
             `try`, put the query after `--`: `-t -- try ...`"
    );
}

#[test]
fn resolve_config_reads_an_unknown_no_tool_word_as_query_text() {
    let tmp = Utf8TempDir::new().unwrap();
    let mut workspace = Workspace::in_memory(tmp.path());
    workspace.load_conversation_index();
    let base = Arc::new(AppConfig::new_test());
    let conversation_id = make_id(4003);
    workspace.create_conversation_with_id(
        conversation_id,
        Conversation::default(),
        Arc::clone(&base),
    );
    let id = conversation_id.to_string();
    let cli = Cli::try_parse_from(["jp", "query", "--id", &id, "-T", "try", "again"]).unwrap();

    let resolved = resolve_config(
        cli.command,
        || Ok(base.to_partial()),
        &cli.globals.config,
        &mut workspace,
        None,
        None,
        false,
    )
    .unwrap();

    let Commands::Query(query) = resolved.command else {
        panic!("parsed as a query");
    };
    assert_eq!(query.query, words(&["try", "again"]));
}

/// A prompt the user ends with Ctrl-C.
struct InterruptedPrompts;

impl PromptBackend for InterruptedPrompts {
    fn inline_select(
        &self,
        _message: &str,
        _options: Vec<InlineOption>,
        _default: Option<char>,
        _writer: &mut dyn io::Write,
    ) -> std::result::Result<char, InquireError> {
        Err(InquireError::OperationInterrupted)
    }

    fn inline_reply(
        &self,
        _message: &str,
        _initial_text: &str,
        _edit_mode: ReplyEditMode,
        _editor_escape: bool,
        _help: Option<&str>,
        _output: Box<dyn io::Write + Send>,
    ) -> std::result::Result<ReplyOutcome, InquireError> {
        Err(InquireError::OperationInterrupted)
    }

    fn select(
        &self,
        _message: &str,
        _options: Vec<String>,
        _default: Option<usize>,
        _writer: &mut dyn io::Write,
    ) -> std::result::Result<usize, InquireError> {
        Err(InquireError::OperationInterrupted)
    }

    fn password(
        &self,
        _message: &str,
        _writer: &mut dyn io::Write,
    ) -> std::result::Result<String, InquireError> {
        Err(InquireError::OperationInterrupted)
    }
}

// Ctrl-C at the question ends the run the way the shell would have, rather
// than reading as a refusal with a config error's exit status.
#[test]
fn ctrl_c_at_the_tool_question_carries_the_interrupt_status() {
    let Err(error) = try_settled_query(
        &["-t", "try", "again"],
        &make_partial_with_tools(),
        &ArgPrompts::new(Arc::new(InterruptedPrompts)),
    ) else {
        panic!("an interrupted question must not settle");
    };

    let Ok(error) = error.downcast::<cmd::Error>() else {
        panic!("an interrupt must stay a typed command error");
    };
    assert_eq!(error.code.get(), 130);
}

#[test]
fn a_word_after_fork_that_is_no_number_starts_the_query() {
    let query = parse_query(&["--fork", "what", "if", "we"]).unwrap();

    assert_eq!(query.fork, Some(None));
    assert_eq!(query.query, words(&["what", "if", "we"]));
}

#[test]
fn a_number_after_fork_stays_the_turn_count() {
    let query = parse_query(&["--fork", "2", "hello"]).unwrap();

    assert_eq!(query.fork, Some(Some(2)));
    assert_eq!(query.query, words(&["hello"]));
}

// `=` says the value is the flag's, so a word that is no number is an error
// rather than the start of the query.
#[test]
fn an_attached_word_after_fork_is_rejected() {
    let Err(error) = parse_query(&["--fork=what", "if", "we"]) else {
        panic!("an attached value must be the flag's");
    };

    assert_eq!(
        error.render().to_string(),
        "error: invalid value 'what' for '--fork [<N>]': expected a positive integer\n\nFor more \
         information, try '--help'.\n"
    );
}

/// How `--fork` rejects the value `x`.
const FORK_REJECTS_X: &str = "error: invalid value 'x' for '--fork [<N>]': expected a positive \
                              integer\n\nFor more information, try '--help'.\n";

// Nothing follows the value, so it can only be the flag's, and a typo is
// reported.
#[test]
fn a_lone_word_after_fork_is_still_rejected() {
    let Err(error) = parse_query(&["--fork", "x"]) else {
        panic!("a value with no query after it must be the flag's");
    };

    assert_eq!(error.render().to_string(), FORK_REJECTS_X);
}

// Words after `--` are never the query directly after a flag, so `--` keeps
// the value the flag's, even with query words elsewhere on the line.
#[test]
fn a_word_after_fork_followed_by_escaped_words_is_still_rejected() {
    let Err(error) = parse_query(&["please", "--fork", "x", "--", "hello"]) else {
        panic!("a value followed by `--` must be the flag's");
    };

    assert_eq!(error.render().to_string(), FORK_REJECTS_X);
}

#[test]
fn a_word_after_tmp_that_is_no_duration_starts_the_query() {
    let query = parse_query(&["-n", "--tmp", "quick", "question"]).unwrap();

    assert_eq!(query.expires_in, Some(None));
    assert_eq!(query.query, words(&["quick", "question"]));
}

#[test]
fn a_duration_after_tmp_stays_the_duration() {
    let query = parse_query(&["-n", "--tmp", "1h", "hello"]).unwrap();

    assert_eq!(query.expires_in, Some(Some("1h".parse().unwrap())));
    assert_eq!(query.query, words(&["hello"]));
}

#[test]
fn an_attached_word_after_tmp_is_rejected() {
    assert!(parse_query(&["-n", "--tmp=quick", "question"]).is_err());
}

#[test]
fn a_word_after_compact_that_is_no_spec_starts_the_query() {
    let query = parse_query(&["-k", "what", "did", "we", "decide"]).unwrap();

    assert!(query.compact.use_config_rules);
    assert_eq!(query.compact.specs, [] as [CompactSpec; 0]);
    assert_eq!(query.query, words(&["what", "did", "we", "decide"]));
}

#[test]
fn a_spec_after_compact_stays_a_spec() {
    let query = parse_query(&["-k", "r+t", "hello"]).unwrap();

    assert!(!query.compact.use_config_rules);
    assert_eq!(query.compact.specs, ["r+t".parse().unwrap()]);
    assert_eq!(query.query, words(&["hello"]));
}

// `-kwhat` shares the flag's argument, which says the value is the flag's.
#[test]
fn an_attached_word_after_compact_is_rejected() {
    let Err(error) = parse_query(&["-kwhat", "did", "we", "decide"]) else {
        panic!("an attached value must be the flag's");
    };

    assert_eq!(
        error.render().to_string(),
        "error: invalid value 'what' for '--compact [<compact>]': unknown policy 'what'\n\nFor \
         more information, try '--help'.\n"
    );
}

#[test]
fn a_word_after_id_that_is_no_target_opens_the_picker() {
    let query = parse_query(&["-i", "fix", "the", "bug"]).unwrap();

    assert_eq!(query.target.ids(), [ConversationTarget::Picker(
        PickerFilter::default()
    )]);
    assert_eq!(query.query, words(&["fix", "the", "bug"]));
}

#[test]
fn a_keyword_after_id_stays_the_target() {
    let query = parse_query(&["-i", "recent", "hello"]).unwrap();

    assert_eq!(query.target.ids(), [ConversationTarget::Recent]);
    assert_eq!(query.query, words(&["hello"]));
}

// `a` is the documented alias for the most recently archived conversation, and
// stays one even where it reads as English.
#[test]
fn the_archived_alias_after_id_stays_the_target() {
    let query = parse_query(&["-i", "a", "quick", "question"]).unwrap();

    assert_eq!(query.target.ids(), [ConversationTarget::Archived]);
    assert_eq!(query.query, words(&["quick", "question"]));
}

#[test]
fn help_after_id_starts_the_query() {
    let query = parse_query(&["-i", "help", "me"]).unwrap();

    assert_eq!(query.target.ids(), [ConversationTarget::Picker(
        PickerFilter::default()
    )]);
    assert_eq!(query.query, words(&["help", "me"]));
}

// `--` is one way to write a title search and a query together.
#[test]
fn a_title_search_after_id_followed_by_escaped_words_stays_the_target() {
    let query = parse_query(&["-i", "auth", "--", "fix", "the", "bug"]).unwrap();

    assert_eq!(query.target.ids(), [ConversationTarget::Picker(
        PickerFilter {
            query: Some("auth".to_owned()),
            ..Default::default()
        }
    )]);
    assert_eq!(query.query, words(&["fix", "the", "bug"]));
}

// `=` is the other: the value is the flag's, wherever it sits.
#[test]
fn an_attached_title_search_after_id_stays_the_target() {
    let query = parse_query(&["-i=auth", "fix", "the", "bug"]).unwrap();

    assert_eq!(query.target.ids(), [ConversationTarget::Picker(
        PickerFilter {
            query: Some("auth".to_owned()),
            ..Default::default()
        }
    )]);
    assert_eq!(query.query, words(&["fix", "the", "bug"]));
}

// Without query text after it, the value is the flag's: a title search.
#[test]
fn a_lone_word_after_id_stays_a_title_search() {
    let query = parse_query(&["-i", "auth"]).unwrap();

    assert_eq!(query.target.ids(), [ConversationTarget::Picker(
        PickerFilter {
            query: Some("auth".to_owned()),
            ..Default::default()
        }
    )]);
    assert_eq!(query.query, None);
}

// Several flags can give a word back in one command line; each lands where it
// was written, between the query words around it.
#[test]
fn words_given_back_by_several_flags_keep_their_places() {
    let query = parse_query(&["-k", "now", "please", "--fork", "then", "go"]).unwrap();

    assert!(query.compact.use_config_rules);
    assert_eq!(query.fork, Some(None));
    assert_eq!(query.query, words(&["now", "please", "then", "go"]));
}

// A flag after the value means no query word follows it, so the value is the
// flag's and a typo is reported.
#[test]
fn a_word_followed_by_another_flag_is_still_rejected() {
    let Err(error) = parse_query(&["-k", "now", "--fork", "then", "go"]) else {
        panic!("a value followed by a flag must be the flag's");
    };

    assert_eq!(
        error.render().to_string(),
        "error: invalid value 'now' for '--compact [<compact>]': unknown policy 'now'\n\nFor more \
         information, try '--help'.\n"
    );
}

#[test]
fn a_word_after_tool_use_naming_no_tool_starts_the_query() {
    let query = settled_query(&["-u", "fix", "the", "bug"], &make_partial_with_tools());

    assert_eq!(query.tool_use, Some(None));
    assert_eq!(
        query.effective_tool_choice(&AppConfig::new_test()),
        ToolChoice::Required
    );
    assert_eq!(query.query, words(&["fix", "the", "bug"]));
}

// A disabled tool can still be forced, so it counts as a tool name.
#[test]
fn a_word_after_tool_use_naming_a_tool_stays_the_tool() {
    let query = settled_query(
        &["-u", "explicitly_disabled_tool", "go"],
        &make_partial_with_tools(),
    );

    assert_eq!(
        query.effective_tool_choice(&AppConfig::new_test()),
        ToolChoice::Function("explicitly_disabled_tool".into())
    );
    assert_eq!(query.query, words(&["go"]));
}

#[test]
fn false_after_tool_use_stays_the_flag_value() {
    let query = settled_query(
        &["-u", "false", "positives", "hurt"],
        &make_partial_with_tools(),
    );

    assert_eq!(
        query.effective_tool_choice(&AppConfig::new_test()),
        ToolChoice::None
    );
    assert_eq!(query.query, words(&["positives", "hurt"]));
}

// Attached, the value names a tool, so an unknown one reaches the check that
// reports it.
#[test]
fn an_attached_word_after_tool_use_stays_the_tool() {
    let query = settled_query(&["-u=fix", "the", "bug"], &make_partial_with_tools());

    let error =
        IntoPartialAppConfig::apply_cli_config(&query, None, make_partial_with_tools(), None)
            .unwrap_err();

    assert_eq!(
        error.to_string(),
        "tool choice 'fix' does not match any known tools"
    );
    assert_eq!(query.query, words(&["the", "bug"]));
}

#[test]
fn a_lone_unknown_word_after_tool_use_is_still_rejected() {
    let query = settled_query(&["-u", "typo"], &make_partial_with_tools());

    let error =
        IntoPartialAppConfig::apply_cli_config(&query, None, make_partial_with_tools(), None)
            .unwrap_err();

    assert_eq!(
        error.to_string(),
        "tool choice 'typo' does not match any known tools"
    );
}

/// How `-t` rejects an empty value.
const EMPTY_TOOL: &str = "error: invalid value '' for '--tool [<tools>]': empty tool name in \
                          ''\n\nFor more information, try '--help'.\n";

// `-t "$TOOL"` with `TOOL` unset must not read as a bare `-t`, which enables
// every tool, nor skip the question a bare `-t` before the query asks.
#[test]
fn an_empty_tool_value_is_rejected() {
    for args in [
        &["-t", "", "explain"][..],
        &["-t=", "explain"][..],
        &["-t", ""][..],
    ] {
        assert_eq!(
            parse_query(args).unwrap_err().render().to_string(),
            EMPTY_TOOL,
            "{args:?}"
        );
    }

    assert_eq!(
        parse_query(&["-T", "", "explain"])
            .unwrap_err()
            .render()
            .to_string(),
        "error: invalid value '' for '--no-tool [<no_tools>]': empty tool name in ''\n\nFor more \
         information, try '--help'.\n"
    );
}

// An empty value is no word of the query, so `--fork "" hello` is a mistake to
// report, not a fork of every turn asking " hello".
#[test]
fn an_empty_fork_value_is_rejected() {
    for args in [&["--fork", "", "hello"][..], &["--fork=", "hello"][..]] {
        assert_eq!(
            parse_query(args).unwrap_err().render().to_string(),
            "error: invalid value '' for '--fork [<N>]': expected a positive integer\n\nFor more \
             information, try '--help'.\n",
            "{args:?}"
        );
    }
}

#[test]
fn empty_name_between_commas_is_rejected() {
    let error = parse_query(&["-tfoo,,bar"]).unwrap_err();

    assert!(
        error.to_string().contains("empty tool name in 'foo,,bar'"),
        "unexpected message: {error}"
    );
}
