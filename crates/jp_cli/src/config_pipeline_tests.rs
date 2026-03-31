use jp_config::{PartialAppConfig, assignment::KvAssignment, conversation::DefaultConversationId};

use super::*;

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
