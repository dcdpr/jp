use assert_matches::assert_matches;
use camino::Utf8Path;
use jp_config::{
    PartialAppConfig, PartialConfig as _, assignment::KvAssignment,
    conversation::DefaultConversationId,
};

use super::*;
use crate::CfgKeyword;

fn empty_pipeline() -> ConfigPipeline {
    ConfigPipeline {
        base: PartialAppConfig::default(),
        cfg_args: vec![],
    }
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
