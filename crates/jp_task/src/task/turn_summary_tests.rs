use std::{sync::Arc, time::Duration};

use chrono::{DateTime, Utc};
use jp_config::model::{
    ModelConfig,
    id::{ModelIdConfig, ModelIdOrAliasConfig, ProviderId},
    parameters::ParametersConfig,
};
use jp_conversation::{
    Conversation, ConversationEvent, EventKind,
    event::{ChatRequest, ChatResponse, TurnStart},
    stream::ConversationEventWithConfig,
};
use jp_llm::{
    StreamErrorKind,
    event::{Event, FinishReason},
    provider::mock::MockProvider,
};
use jp_workspace::ConversationLock;

use super::*;
use crate::TaskHandler;

const TURN_1_SUMMARY: &str = r#"{"summaries":{"turn001":"Ask the first question"}}"#;
const UNSET_KEY: &str = "JP_TASK_TEST_TURN_SUMMARY_UNSET_API_KEY";
const BATCH_SIZE: usize = 5;
const SYNC_TIMEOUT: Duration = Duration::from_secs(10);

fn conversation_id() -> ConversationId {
    "jp-c17866928997".parse().expect("valid id")
}

fn push_turn(events: &mut ConversationStream, n: usize, question: &str, answer: &str) {
    let config = events.base_config().to_partial();
    let entry = |id: String, kind: EventKind| ConversationEventWithConfig {
        event_id: EventId::fixed(&id),
        event: ConversationEvent::new(kind, DateTime::<Utc>::UNIX_EPOCH),
        config: config.clone(),
    };

    events.extend([
        entry(format!("turn{n:03}"), TurnStart::default().into()),
        entry(format!("chat{n:03}"), ChatRequest::from(question).into()),
        entry(format!("reply{n:02}"), ChatResponse::message(answer).into()),
    ]);
}

fn model(provider: ProviderId, name: &str) -> ModelConfig {
    ModelConfig {
        id: ModelIdOrAliasConfig::Id(ModelIdConfig {
            provider,
            name: name.parse().expect("valid model name"),
        }),
        parameters: ParametersConfig::default(),
    }
}

fn summarizing_with(model: ModelConfig) -> AppConfig {
    let mut config = AppConfig::new_test();
    config.conversation.turn_summary.generate.model = Some(model);
    config
}

fn summarizing_with_a_broken_provider() -> AppConfig {
    let mut config = summarizing_with(model(ProviderId::Cerebras, "model"));
    config.providers.llm.cerebras.api_key_env = UNSET_KEY.to_owned().into();
    config
}

fn task(
    conversation_id: ConversationId,
    events: &ConversationStream,
    provider: MockProvider,
) -> TurnSummaryTask {
    TurnSummaryTask {
        conversation_id,
        provider: Arc::new(provider),
        request: Some(TurnSummaryRequest {
            events: events.clone(),
            turn_ids: unsummarized_turns(events),
            model: model(ProviderId::Test, "mock"),
            batch_size: BATCH_SIZE,
            max_response_bytes: None,
            notices: NoticeSink::new(|_| {}),
        }),
        summaries: vec![],
    }
}

fn answering(data: &str) -> MockProvider {
    MockProvider::new(vec![
        Event::structured(0, data),
        Event::flush(0),
        Event::Finished(FinishReason::Completed),
    ])
}

fn conversation(
    workspace: &mut Workspace,
    build: impl FnOnce(&mut ConversationStream),
) -> ConversationId {
    let lock = workspace
        .create_and_lock_conversation_with_id(
            conversation_id(),
            Conversation::default(),
            Arc::new(AppConfig::new_test()),
            None,
        )
        .unwrap();

    let mut conv = lock.into_mut();
    conv.update_events(build);
    conv.flush().unwrap();
    conv.id()
}

fn lock(workspace: &Workspace, id: ConversationId) -> ConversationLock {
    let handle = workspace.acquire_conversation(&id).unwrap();
    match workspace.lock_conversation(handle, None).unwrap() {
        LockResult::Acquired(lock) => lock,
        LockResult::AlreadyLocked(_) => panic!("conversation is already locked"),
    }
}

fn update(workspace: &Workspace, id: ConversationId, edit: impl FnOnce(&mut ConversationStream)) {
    let mut conv = lock(workspace, id).into_mut();
    conv.update_events(edit);
    conv.flush().unwrap();
}

fn stored(workspace: &Workspace, id: ConversationId) -> ConversationStream {
    lock(workspace, id).events().clone()
}

fn summaries(workspace: &Workspace, id: ConversationId) -> Vec<Option<String>> {
    stored(workspace, id)
        .iter_turns()
        .map(|turn| turn.summary().map(str::to_owned))
        .collect()
}

#[test]
fn new_offers_every_turn_without_a_summary() {
    let mut events = ConversationStream::new_test();
    push_turn(&mut events, 1, "First question", "First answer");
    push_turn(&mut events, 2, "Second question", "Second answer");
    push_turn(&mut events, 3, "Third question", "Third answer");
    assert!(events.set_turn_summary(&EventId::fixed("turn002"), "Already summarized"));
    let config = summarizing_with(model(ProviderId::Test, "mock"));

    let task = TurnSummaryTask::new(conversation_id(), &events, &config, NoticeSink::new(|_| {}))
        .unwrap()
        .expect("two turns have no summary");

    let request = task.request.expect("task carries a request");
    assert_eq!(request.turn_ids, [
        EventId::fixed("turn001"),
        EventId::fixed("turn003")
    ]);
    assert_eq!(request.batch_size, BATCH_SIZE);
}

#[test]
fn new_does_not_set_up_a_provider_when_every_turn_has_a_summary() {
    let mut events = ConversationStream::new_test();
    push_turn(&mut events, 1, "First question", "First answer");
    assert!(events.set_turn_summary(&EventId::fixed("turn001"), "Already summarized"));

    let task = TurnSummaryTask::new(
        conversation_id(),
        &events,
        &summarizing_with_a_broken_provider(),
        NoticeSink::new(|_| {}),
    )
    .expect("provider is not initialized when no turns need summaries");

    assert!(task.is_none());
}

#[test]
fn new_offers_nothing_with_a_batch_size_of_zero() {
    let mut events = ConversationStream::new_test();
    push_turn(&mut events, 1, "First question", "First answer");
    let mut config = summarizing_with_a_broken_provider();
    config.conversation.turn_summary.generate.batch_size = 0;

    let task = TurnSummaryTask::new(conversation_id(), &events, &config, NoticeSink::new(|_| {}))
        .expect("provider is not initialized when batch_size is 0");

    assert!(task.is_none());
}

#[test]
fn new_offers_nothing_for_a_conversation_without_turns() {
    let task = TurnSummaryTask::new(
        conversation_id(),
        &ConversationStream::new_test(),
        &summarizing_with_a_broken_provider(),
        NoticeSink::new(|_| {}),
    )
    .unwrap();

    assert!(task.is_none());
}

#[test]
fn new_fails_when_the_summary_provider_cannot_be_set_up() {
    let mut events = ConversationStream::new_test();
    push_turn(&mut events, 1, "First question", "First answer");

    let Err(error) = TurnSummaryTask::new(
        conversation_id(),
        &events,
        &summarizing_with_a_broken_provider(),
        NoticeSink::new(|_| {}),
    ) else {
        panic!("provider without API key should fail initialization");
    };

    assert!(error.to_string().contains(UNSET_KEY), "got: {error}");
}

#[tokio::test]
async fn a_turn_is_summarized_during_the_turn_after_it() {
    let mut workspace = Workspace::in_memory("root");
    let id = conversation(&mut workspace, |events| {
        push_turn(events, 1, "First question", "First answer");
    });

    let mut handler = TaskHandler::default();
    handler.spawn(task(id, &stored(&workspace, id), answering(TURN_1_SUMMARY)));

    update(&workspace, id, |events| {
        push_turn(events, 2, "Second question", "Second answer");
    });

    handler.sync(&mut workspace, SYNC_TIMEOUT).await.unwrap();

    assert_eq!(summaries(&workspace, id), [
        Some("Ask the first question".to_owned()),
        None
    ]);
}

#[tokio::test]
async fn a_turn_edited_before_the_task_syncs_is_left_without_a_summary() {
    let mut workspace = Workspace::in_memory("root");
    let id = conversation(&mut workspace, |events| {
        push_turn(events, 1, "First question", "First answer");
    });

    let mut handler = TaskHandler::default();
    handler.spawn(task(id, &stored(&workspace, id), answering(TURN_1_SUMMARY)));

    update(&workspace, id, |events| {
        for mut event in events.iter_mut() {
            if let Some(request) = event.as_chat_request_mut() {
                request.content = "An edited question".to_owned();
            }
        }
    });

    handler.sync(&mut workspace, SYNC_TIMEOUT).await.unwrap();

    assert_eq!(summaries(&workspace, id), [None]);
}

#[tokio::test]
async fn a_summary_stored_before_the_task_syncs_is_kept() {
    let mut workspace = Workspace::in_memory("root");
    let id = conversation(&mut workspace, |events| {
        push_turn(events, 1, "First question", "First answer");
    });

    let mut handler = TaskHandler::default();
    handler.spawn(task(id, &stored(&workspace, id), answering(TURN_1_SUMMARY)));

    update(&workspace, id, |events| {
        assert!(events.set_turn_summary(&EventId::fixed("turn001"), "Written by hand"));
    });

    handler.sync(&mut workspace, SYNC_TIMEOUT).await.unwrap();

    assert_eq!(summaries(&workspace, id), [Some(
        "Written by hand".to_owned()
    )]);
}

#[tokio::test]
async fn a_turn_removed_before_the_task_syncs_gets_no_summary() {
    let mut workspace = Workspace::in_memory("root");
    let id = conversation(&mut workspace, |events| {
        push_turn(events, 1, "First question", "First answer");
        push_turn(events, 2, "Second question", "Second answer");
    });

    let mut handler = TaskHandler::default();
    handler.spawn(task(id, &stored(&workspace, id), answering(TURN_1_SUMMARY)));

    update(&workspace, id, |events| {
        events.retain_turns(|turn| turn == 1);
    });

    handler.sync(&mut workspace, SYNC_TIMEOUT).await.unwrap();

    let events = stored(&workspace, id);
    assert_eq!(
        events
            .iter_turns()
            .map(|turn| turn.id().cloned())
            .collect::<Vec<_>>(),
        [Some(EventId::fixed("turn002"))]
    );
    assert_eq!(summaries(&workspace, id), [None]);
}

#[tokio::test]
async fn a_conversation_locked_elsewhere_is_left_alone() {
    let mut workspace = Workspace::in_memory("root");
    let id = conversation(&mut workspace, |events| {
        push_turn(events, 1, "First question", "First answer");
    });

    let mut handler = TaskHandler::default();
    handler.spawn(task(id, &stored(&workspace, id), answering(TURN_1_SUMMARY)));

    let held = lock(&workspace, id);
    handler.sync(&mut workspace, SYNC_TIMEOUT).await.unwrap();
    drop(held);

    assert_eq!(summaries(&workspace, id), [None]);
}

#[tokio::test]
async fn a_failed_summary_request_stores_nothing() {
    let mut workspace = Workspace::in_memory("root");
    let id = conversation(&mut workspace, |events| {
        push_turn(events, 1, "First question", "First answer");
    });
    let before = stored(&workspace, id);

    let mut handler = TaskHandler::default();
    handler.spawn(task(
        id,
        &before,
        MockProvider::with_stream_error(StreamErrorKind::Other, "boom"),
    ));

    handler.sync(&mut workspace, SYNC_TIMEOUT).await.unwrap();

    assert_eq!(stored(&workspace, id), before);
}

#[tokio::test]
async fn generate_returns_the_summaries_for_its_conversation() {
    let mut events = ConversationStream::new_test();
    push_turn(&mut events, 1, "First question", "First answer");

    let summaries = task(conversation_id(), &events, answering(TURN_1_SUMMARY))
        .generate()
        .await
        .unwrap();

    assert_eq!(summaries, TurnSummaries {
        conversation_id: conversation_id(),
        summaries: vec![generated(&events, "Ask the first question")],
    });
}

#[tokio::test]
async fn summaries_are_stored_through_a_lock_the_caller_holds() {
    let mut workspace = Workspace::in_memory("root");
    let id = conversation(&mut workspace, |events| {
        push_turn(events, 1, "First question", "First answer");
    });
    let pending = task(id, &stored(&workspace, id), answering(TURN_1_SUMMARY))
        .generate()
        .await
        .unwrap();

    let held = lock(&workspace, id);
    pending.store(&held);

    let handle = workspace.acquire_conversation(&id).unwrap();
    assert!(matches!(
        workspace.lock_conversation(handle, None).unwrap(),
        LockResult::AlreadyLocked(_)
    ));

    drop(held);
    assert_eq!(summaries(&workspace, id), [Some(
        "Ask the first question".to_owned()
    )]);
}

fn generated(events: &ConversationStream, summary: &str) -> GeneratedTurnSummary {
    let transcript = turn_summary::transcripts(events, &[EventId::fixed("turn001")])
        .pop()
        .expect("turn 1 has a transcript");

    GeneratedTurnSummary {
        turn_id: transcript.turn_id().clone(),
        summary: summary.to_owned(),
        fingerprint: transcript.fingerprint(),
    }
}

#[test]
fn a_summary_of_an_unchanged_turn_is_current() {
    let mut events = ConversationStream::new_test();
    push_turn(&mut events, 1, "First question", "First answer");
    let summary = generated(&events, "Ask the first question");

    push_turn(&mut events, 2, "Second question", "Second answer");

    assert_eq!(still_current(&events, vec![summary.clone()]), [summary]);
}

#[test]
fn a_summary_of_an_edited_turn_is_not_current() {
    let mut events = ConversationStream::new_test();
    push_turn(&mut events, 1, "First question", "First answer");
    let summary = generated(&events, "Ask the first question");

    for mut event in events.iter_mut() {
        if let Some(response) = event.as_chat_response_mut() {
            *response = ChatResponse::message("An edited answer");
        }
    }

    assert!(still_current(&events, vec![summary]).is_empty());
}
