use assert_matches::assert_matches;
use camino::Utf8Path;
use jp_config::{
    PartialAppConfig, PartialConfig as _,
    assignment::KvAssignment,
    conversation::DefaultConversationId,
    providers::mcp::{PartialMcpProviderConfig, PartialStdioConfig},
};

use super::*;
use crate::CfgKeyword;

fn empty_pipeline() -> ConfigPipeline {
    ConfigPipeline {
        base: PartialAppConfig::default(),
        cfg_args: vec![],
    }
}

/// The accumulated partial of a conversation whose `bash` tool grants the named
/// filesystem paths.
///
/// Mirrors `ConversationStream::config_partial` on a conversation with no
/// deltas: the base config's own partial.
fn partial_with_fs_rules(paths: &[&str]) -> PartialAppConfig {
    config_with_fs_rules(paths).to_partial()
}

/// A config whose `bash` tool grants the named filesystem paths.
fn config_with_fs_rules(paths: &[&str]) -> jp_config::AppConfig {
    use jp_config::conversation::tool::{
        PartialToolConfig, ToolSource,
        access::{PartialAccessConfig, PartialFsRuleConfig},
    };

    let mut partial = PartialAppConfig::new_test();
    partial
        .conversation
        .tools
        .tools
        .insert("bash".to_owned(), PartialToolConfig {
            source: Some(ToolSource::Local { tool: None }),
            access: Some(PartialAccessConfig {
                fs: paths
                    .iter()
                    .map(|path| PartialFsRuleConfig {
                        path: Some((*path).to_owned()),
                        read: Some(true),
                        ..PartialFsRuleConfig::default()
                    })
                    .collect(),
                env: jp_config::types::vec::MergeableVec::default(),
            }),
            ..PartialToolConfig::default()
        });

    jp_config::util::build(partial).expect("valid config")
}

// `override_to_record` names no field and knows no merge strategy: it merges
// two partials and compares them. These tests therefore cover the axes a field
// can vary along — how it merges, whether the override changes its value, and
// whether it changes only the metadata deciding the next merge — rather than
// particular fields. Each picks a field that behaves the way the case needs.

/// The accumulated partial of a conversation with no deltas.
fn base_partial() -> PartialAppConfig {
    jp_config::util::build(PartialAppConfig::new_test())
        .unwrap()
        .to_partial()
}

/// A field that merges by replacement, restated at its current value.
#[test]
fn an_override_matching_the_current_scalar_records_nothing() {
    let current = base_partial();

    let mut overrides = PartialAppConfig::empty();
    overrides.conversation.start_local = current.conversation.start_local;

    assert_eq!(override_to_record(&current, overrides).unwrap(), None);
}

/// A field that merges by replacement, given a different value.
#[test]
fn an_override_changing_a_scalar_is_recorded() {
    let current = base_partial();

    let mut overrides = PartialAppConfig::empty();
    overrides.conversation.start_local = Some(!current.conversation.start_local.unwrap_or(false));

    assert!(override_to_record(&current, overrides).unwrap().is_some());
}

/// A list carrying its own merge strategy, restated at its current value.
///
/// Such a list is the same shape whether the element it holds is already there
/// or not, so only merging tells the two apart — which is why the comparison
/// is not made against the override itself.
///
/// The override is scoped to the subtree under test rather than being a whole
/// snapshot: a snapshot layered over itself appends every list that merges by
/// appending without deduplicating to itself, which is a real change.
/// See `an_override_repeating_an_appending_list_is_recorded`.
#[test]
fn an_override_restating_an_existing_rule_records_nothing() {
    let current = partial_with_fs_rules(&["src"]);

    let mut overrides = PartialAppConfig::empty();
    overrides.conversation.tools = partial_with_fs_rules(&["src"]).conversation.tools;

    assert_eq!(override_to_record(&current, overrides).unwrap(), None);
}

/// A list that merges by appending, restated at a value it already holds.
///
/// MCP arguments preserve repetition.
/// Appending without deduplicating is not idempotent: asking for `FOO` twice
/// leaves the list holding it twice, which is a different config and so a real
/// change to record.
/// A caller that means to set such a list rather than grow it says `replace`.
#[test]
fn an_override_repeating_an_appending_list_is_recorded() {
    let current = base_partial();

    let mut overrides = PartialAppConfig::empty();
    overrides
        .providers
        .mcp
        .insert("bookworm".to_owned(), mcp_server("FOO"));

    assert!(
        override_to_record(&current, overrides.clone())
            .unwrap()
            .is_some()
    );

    // Again, against a partial that already holds it.
    let mut current = current;
    current.merge(&(), overrides.clone()).unwrap();

    assert!(override_to_record(&current, overrides).unwrap().is_some());
}

/// A dedup policy the conversation already carries governs the comparison.
///
/// Resolution reduces `assistant.system_prompt` to its text, so reading the
/// conversation as a resolved config loses the policy and tests the append
/// under the default `block` mode, where the block counts as already present.
/// Read from the accumulated partial, `off` is in force and the append lands.
#[test]
fn an_override_is_compared_under_the_conversations_dedup_policy() {
    use jp_config::types::string::{PartialMergeableString, PartialMergedString, StringDedup};

    let mut current = base_partial();
    current.assistant.system_prompt = Some(PartialMergeableString::Merged(PartialMergedString {
        value: Some("A".to_owned()),
        dedup: Some(StringDedup::Off),
        ..PartialMergedString::default()
    }));

    let mut overrides = PartialAppConfig::empty();
    overrides.assistant.system_prompt = Some(PartialMergeableString::Merged(PartialMergedString {
        value: Some("A".to_owned()),
        strategy: Some(jp_config::types::string::MergedStringStrategy::Append),
        ..PartialMergedString::default()
    }));

    assert!(
        override_to_record(&current, overrides).unwrap().is_some(),
        "`dedup = off` lets the block land a second time"
    );
}

/// A list carrying its own merge strategy, given an element it lacks.
#[test]
fn an_override_adding_a_rule_is_recorded() {
    let current = partial_with_fs_rules(&["src"]);

    let mut overrides = PartialAppConfig::empty();
    overrides.conversation.tools = partial_with_fs_rules(&["src", "docs"]).conversation.tools;

    assert!(override_to_record(&current, overrides).unwrap().is_some());
}

#[test]
fn without_conversation_returns_base_plus_cfg() {
    let mut pipeline = empty_pipeline();
    pipeline.cfg_args.push(ResolvedCfgArg::KeyValue(
        "conversation.default_id=last"
            .parse::<KvAssignment>()
            .unwrap(),
    ));

    let partial = pipeline.partial_without_conversation().unwrap();
    assert_eq!(
        partial.conversation.default_id,
        Some(DefaultConversationId::LastActivated)
    );
}

#[test]
fn with_conversation_preserves_cfg_over_conversation() {
    let mut pipeline = empty_pipeline();
    pipeline.cfg_args.push(ResolvedCfgArg::KeyValue(
        "conversation.start_local=true"
            .parse::<KvAssignment>()
            .unwrap(),
    ));

    // Conversation layer sets start_local = false
    let mut conv = PartialAppConfig::empty();
    conv.conversation.start_local = Some(false);

    let partial = pipeline.partial_with_conversation(conv).unwrap();
    // `--cfg` should win over conversation layer
    assert_eq!(partial.conversation.start_local, Some(true));
}

#[test]
fn conversation_layer_overrides_base() {
    let pipeline = empty_pipeline();

    let mut conv = PartialAppConfig::empty();
    conv.conversation.start_local = Some(true);

    let partial = pipeline.partial_with_conversation(conv).unwrap();
    assert_eq!(partial.conversation.start_local, Some(true));
}

/// An MCP server entry with a command and one argument.
fn mcp_server(argument: &str) -> PartialMcpProviderConfig {
    PartialMcpProviderConfig::Stdio(PartialStdioConfig {
        command: Some("just".into()),
        arguments: Some(vec![argument.to_owned()].into()),
        ..PartialStdioConfig::default()
    })
}

/// The `arguments` of a server in a resolved partial.
fn mcp_arguments(partial: &PartialAppConfig, server: &str) -> Option<Vec<String>> {
    let PartialMcpProviderConfig::Stdio(config) = partial.providers.mcp.get(server)?;
    config.arguments.as_deref().cloned()
}

/// The per-conversation layer is a resolved snapshot, not a contribution.
///
/// Applying it over the file layer must not re-run a field's combining merge
/// strategy, or every field that merges by appending doubles: the conversation
/// was resolved from the same files, so both layers carry the same list.
#[test]
fn conversation_layer_does_not_duplicate_an_appended_list() {
    let mut base = PartialAppConfig::empty();
    base.providers
        .mcp
        .insert("bookworm".to_owned(), mcp_server("serve-bookworm"));

    // The conversation was created from this same config.
    let conversation = base.clone();

    let pipeline = ConfigPipeline {
        base,
        cfg_args: vec![],
    };
    let partial = pipeline.partial_with_conversation(conversation).unwrap();

    assert_eq!(
        mcp_arguments(&partial, "bookworm"),
        Some(vec!["serve-bookworm".to_owned()])
    );
}

/// A server the conversation never saw still reaches it.
///
/// The conversation layer shadows the file layer field by field, so a map has
/// to keep the entries only the file layer holds — otherwise adding a server
/// to the workspace config would hide it from every existing conversation.
#[test]
fn conversation_layer_keeps_a_server_only_the_file_layer_has() {
    let mut base = PartialAppConfig::empty();
    base.providers
        .mcp
        .insert("bookworm".to_owned(), mcp_server("serve-bookworm"));
    base.providers
        .mcp
        .insert("kagi".to_owned(), mcp_server("serve-kagi"));

    // The conversation predates the `kagi` entry.
    let mut conversation = PartialAppConfig::empty();
    conversation
        .providers
        .mcp
        .insert("bookworm".to_owned(), mcp_server("serve-bookworm"));

    let pipeline = ConfigPipeline {
        base,
        cfg_args: vec![],
    };
    let partial = pipeline.partial_with_conversation(conversation).unwrap();

    assert_eq!(
        mcp_arguments(&partial, "kagi"),
        Some(vec!["serve-kagi".to_owned()])
    );
    assert_eq!(
        mcp_arguments(&partial, "bookworm"),
        Some(vec!["serve-bookworm".to_owned()])
    );
}

/// Write `content` to `name` inside `root` and return the file's path.
fn write_config(root: &Utf8Path, name: &str, content: &str) -> KeyValueOrPath {
    let path = root.join(name);
    std::fs::write(&path, content).unwrap();
    KeyValueOrPath::Path(path)
}

#[test]
fn entry_loader_reset_discards_accumulated_state() {
    let tmp = camino_tempfile::tempdir().unwrap();
    let entry = write_config(tmp.path(), "committer.toml", indoc::indoc! {r#"
            [loader]
            reset = "none"

            [conversation]
            start_local = true
        "#});

    let mut base = PartialAppConfig::empty();
    base.conversation.title.generate.auto = Some(false);

    let pipeline = ConfigPipeline::new(&[entry], None, None, || Ok(base)).unwrap();
    let partial = pipeline.partial_without_conversation().unwrap();

    // The entry's own contribution survives the reset…
    assert_eq!(partial.conversation.start_local, Some(true));
    // …while the accumulated base state is discarded.
    assert_eq!(partial.conversation.title.generate.auto, None);

    // The reset is reported as a reset point to program defaults, so
    // continuing conversations persist `[Reset, Apply(post)]` ([RFD 038]).
    assert_matches!(pipeline.config_reset(), Some(ConfigReset::Defaults));
}

#[test]
fn entry_without_loader_reset_layers_on_top() {
    let tmp = camino_tempfile::tempdir().unwrap();
    let entry = write_config(tmp.path(), "dev.toml", "conversation.start_local = true");

    let mut base = PartialAppConfig::empty();
    base.conversation.title.generate.auto = Some(false);

    let pipeline = ConfigPipeline::new(&[entry], None, None, || Ok(base)).unwrap();
    let partial = pipeline.partial_without_conversation().unwrap();

    assert_eq!(partial.conversation.start_local, Some(true));
    assert_eq!(partial.conversation.title.generate.auto, Some(false));
    assert_matches!(pipeline.config_reset(), None);
}

#[test]
fn extends_reached_loader_reset_is_ignored() {
    // `loader.reset` is honored only on the explicit entry itself: a
    // transitive reset would let an included fragment discard its parent
    // entry's accumulated config ([RFD 038]).
    let tmp = camino_tempfile::tempdir().unwrap();
    write_config(tmp.path(), "fragment.toml", indoc::indoc! {r#"
            [loader]
            reset = "none"

            [conversation]
            start_local = true
        "#});
    let entry = write_config(tmp.path(), "entry.toml", "extends = [\"fragment.toml\"]");

    let mut base = PartialAppConfig::empty();
    base.conversation.title.generate.auto = Some(false);

    let pipeline = ConfigPipeline::new(&[entry], None, None, || Ok(base)).unwrap();
    let partial = pipeline.partial_without_conversation().unwrap();

    // The fragment's values apply, but its reset directive does not.
    assert_eq!(partial.conversation.start_local, Some(true));
    assert_eq!(partial.conversation.title.generate.auto, Some(false));
    assert_matches!(pipeline.config_reset(), None);

    // The fragment's `[loader]` section does not leak into resolved state.
    assert_eq!(partial.loader.reset, None);
}

#[test]
fn last_reset_point_wins() {
    let tmp = camino_tempfile::tempdir().unwrap();
    let entry = write_config(tmp.path(), "committer.toml", indoc::indoc! {r#"
            [loader]
            reset = "none"
        "#});

    // Entry-local reset followed by `WORKSPACE`: the keyword is effective.
    let pipeline = ConfigPipeline::new(
        &[
            entry.clone(),
            KeyValueOrPath::Keyword(CfgKeyword::Workspace),
        ],
        None,
        None,
        || Ok(PartialAppConfig::empty()),
    )
    .unwrap();
    assert_matches!(pipeline.config_reset(), Some(ConfigReset::Workspace(_)));

    // `WORKSPACE` followed by an entry-local reset: the entry is effective.
    let pipeline = ConfigPipeline::new(
        &[KeyValueOrPath::Keyword(CfgKeyword::Workspace), entry],
        None,
        None,
        || Ok(PartialAppConfig::empty()),
    )
    .unwrap();
    assert_matches!(pipeline.config_reset(), Some(ConfigReset::Defaults));
}

#[test]
fn later_root_reset_discards_earlier_entries_of_same_argument() {
    // When one `--cfg` argument resolves to multiple entries across search
    // roots, a `loader.reset = "none"` on a later entry resets state at that
    // point, discarding earlier entries from the same argument ([RFD 038]).
    let mut first = PartialAppConfig::empty();
    first.conversation.start_local = Some(true);

    let mut second = PartialAppConfig::empty();
    second.conversation.title.generate.auto = Some(false);

    let mut base = PartialAppConfig::empty();
    base.conversation.default_id = Some(DefaultConversationId::LastActivated);

    let pipeline = ConfigPipeline {
        base,
        cfg_args: vec![ResolvedCfgArg::Partials(vec![
            CfgEntry {
                reset: false,
                partial: first,
            },
            CfgEntry {
                reset: true,
                partial: second,
            },
        ])],
    };

    let partial = pipeline.partial_without_conversation().unwrap();

    // The resetting entry's own contribution survives…
    assert_eq!(partial.conversation.title.generate.auto, Some(false));
    // …while the earlier entry from the same argument and the base state are
    // discarded.
    assert_eq!(partial.conversation.start_local, None);
    assert_eq!(partial.conversation.default_id, None);

    assert_matches!(pipeline.config_reset(), Some(ConfigReset::Defaults));
}

#[test]
fn scan_detects_keywords_and_rejects_the_combination() {
    let keywords = scan_cfg_keywords(&[KeyValueOrPath::Keyword(CfgKeyword::None)]).unwrap();
    assert!(keywords.none);
    assert!(!keywords.workspace);

    let err = scan_cfg_keywords(&[
        KeyValueOrPath::Keyword(CfgKeyword::None),
        KeyValueOrPath::Keyword(CfgKeyword::Workspace),
    ])
    .unwrap_err();
    assert!(err.to_string().contains("mutually exclusive"), "{err}");
}

#[test]
fn none_keyword_gates_base_loading_inside_the_pipeline() {
    // The pipeline owns the implicit-loading decision ([RFD 038]): under
    // `--cfg=NONE`, the base loader is never invoked, so broken implicit
    // config cannot prevent the pipeline from being built.
    let pipeline = ConfigPipeline::new(
        &[KeyValueOrPath::Keyword(CfgKeyword::None)],
        None,
        None,
        || panic!("implicit config loading must be skipped under --cfg=NONE"),
    )
    .unwrap();

    // The reset point is program defaults, and nothing leaks into the state.
    assert_matches!(pipeline.config_reset(), Some(ConfigReset::Defaults));
    let partial = pipeline.partial_without_conversation().unwrap();
    assert!(partial.is_empty());
}

#[test]
fn keyword_mutual_exclusion_is_rejected_before_base_loading() {
    // The `NONE`/`WORKSPACE` combination is rejected by the pre-scan, before
    // the pipeline touches any config source.
    let result = ConfigPipeline::new(
        &[
            KeyValueOrPath::Keyword(CfgKeyword::None),
            KeyValueOrPath::Keyword(CfgKeyword::Workspace),
        ],
        None,
        None,
        || panic!("base loading must not run when the keyword scan fails"),
    );
    let err = result
        .err()
        .expect("the keyword combination must be rejected");
    assert!(err.to_string().contains("mutually exclusive"), "{err}");
}

#[test]
fn base_loader_runs_without_the_none_gate() {
    // Without `NONE`, the pipeline invokes the loader and layers `--cfg`
    // directives on top of its result.
    let mut base = PartialAppConfig::empty();
    base.conversation.start_local = Some(true);

    let pipeline = ConfigPipeline::new(&[], None, None, || Ok(base)).unwrap();
    let partial = pipeline.partial_without_conversation().unwrap();
    assert_eq!(partial.conversation.start_local, Some(true));
}

#[test]
fn loader_reset_does_not_trigger_the_none_gate() {
    // `loader.reset = "none"` is positional only: the pre-pipeline gate that
    // skips implicit config loading responds to the `NONE` keyword alone, so
    // broken implicit config still requires `NONE` / `--no-cfg` ([RFD 038]).
    // The gate runs before any file is read, so a resetting entry cannot
    // influence it.
    let tmp = camino_tempfile::tempdir().unwrap();
    let entry = write_config(tmp.path(), "committer.toml", indoc::indoc! {r#"
            [loader]
            reset = "none"
        "#});

    let keywords = scan_cfg_keywords(&[entry]).unwrap();
    assert!(!keywords.none);
    assert!(!keywords.workspace);
}

#[test]
fn loader_assignment_is_rejected() {
    // `loader` is load-time metadata, not application config: it is not an
    // assignable key, so `--cfg loader.reset=none` fails instead of leaking
    // loader state into the resolved partial ([RFD 038]). Only a file entry's
    // own `[loader]` section is honored, at load time.
    let pipeline = ConfigPipeline::new(
        &[KeyValueOrPath::KeyValue(
            "loader.reset=none".parse::<KvAssignment>().unwrap(),
        )],
        None,
        None,
        || Ok(PartialAppConfig::empty()),
    )
    .unwrap();

    let err = pipeline.partial_without_conversation().unwrap_err();
    assert!(err.to_string().contains("unknown key"), "{err}");
    assert_matches!(pipeline.config_reset(), None);
}
