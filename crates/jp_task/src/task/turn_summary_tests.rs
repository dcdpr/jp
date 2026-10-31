use std::{sync::Arc, time::Duration};

use chrono::{DateTime, Utc};
use jp_config::model::{
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

/// What the summary model answers in these tests: a summary for `turn001`.
const TURN_1_SUMMARY: &str =
    r#"{"summaries":[{"turn_id":"turn001","summary":"Ask the first question"}]}"#;

/// An API key variable no environment sets, so a provider reading it cannot be
/// set up.
const UNSET_KEY: &str = "JP_TASK_TEST_TURN_SUMMARY_UNSET_API_KEY";

fn conversation_id() -> ConversationId {
    "jp-c17866928997".parse().expect("valid id")
}

/// Append turn `n`, one question and its answer, with its entries named
/// `turn00n`, `chat00n` and `reply0n`.
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

/// A model on `provider`, with default parameters.
fn model(provider: ProviderId, name: &str) -> ModelConfig {
    ModelConfig {
        id: ModelIdOrAliasConfig::Id(ModelIdConfig {
            provider,
            name: name.parse().expect("valid model name"),
        }),
        parameters: ParametersConfig::default(),
    }
}

/// A config whose turn summaries run on `model`.
fn summarizing_with(model: ModelConfig) -> AppConfig {
    let mut config = AppConfig::new_test();
    config.conversation.turn_summary.generate.model = Some(model);
    config
}

/// A config whose summary model is served by a provider that cannot be set up:
/// its API key variable is unset.
fn summarizing_with_a_broken_provider() -> AppConfig {
    let mut config = summarizing_with(model(ProviderId::Cerebras, "model"));
    config.providers.llm.cerebras.api_key_env = UNSET_KEY.to_owned().into();
    config
}

/// A task for the turns in `events` without a summary, answered by `provider`.
fn task(
    conversation_id: ConversationId,
    events: &ConversationStream,
    provider: MockProvider,
) -> TurnSummaryTask {
    TurnSummaryTask {
        conversation_id,
        events: events.clone(),
        turn_ids: unsummarized_turns(events),
        model: model(ProviderId::Test, "mock"),
        provider: Arc::new(provider),
        batch_size: 5,
        max_response_bytes: None,
        notices: NoticeSink::new(|_| {}),
        summaries: vec![],
    }
}

/// A provider that answers with `data` as structured output.
fn answering(data: &str) -> MockProvider {
    MockProvider::new(vec![
        Event::structured(0, data),
        Event::flush(0),
        Event::Finished(FinishReason::Completed),
    ])
}

/// Create a conversation holding what `build` appends, and write it.
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

/// Lock the conversation, as another `jp` process would.
fn lock(workspace: &Workspace, id: ConversationId) -> ConversationLock {
    let handle = workspace.acquire_conversation(&id).unwrap();
    match workspace.lock_conversation(handle, None).unwrap() {
        LockResult::Acquired(lock) => lock,
        LockResult::AlreadyLocked(_) => panic!("the conversation is already locked"),
    }
}

/// Apply `edit` to the conversation's events and write them.
fn update(workspace: &Workspace, id: ConversationId, edit: impl FnOnce(&mut ConversationStream)) {
    let mut conv = lock(workspace, id).into_mut();
    conv.update_events(edit);
    conv.flush().unwrap();
}

/// The conversation's events, read back from storage.
///
/// Taking the lock reloads the conversation, so what this returns is what was
/// written rather than what is cached.
fn stored(workspace: &Workspace, id: ConversationId) -> ConversationStream {
    lock(workspace, id).events().clone()
}

/// Each stored turn's summary, in turn order.
fn summaries(workspace: &Workspace, id: ConversationId) -> Vec<Option<String>> {
    stored(workspace, id)
        .iter_turns()
        .map(|turn| turn.summary().map(str::to_owned))
        .collect()
}

// ---------------------------------------------------------------------------
// Creating a task
// ---------------------------------------------------------------------------

#[test]
fn new_offers_every_turn_without_a_summary() {
    let mut events = ConversationStream::new_test();
    push_turn(&mut events, 1, "First question", "First answer");
    push_turn(&mut events, 2, "Second question", "Second answer");
    push_turn(&mut events, 3, "Third question", "Third answer");
    assert!(events.set_turn_summary(&EventId::fixed("turn002"), "Already summarized".to_owned()));
    let config = summarizing_with(model(ProviderId::Test, "mock"));

    let task = TurnSummaryTask::new(conversation_id(), &events, &config, NoticeSink::new(|_| {}))
        .unwrap()
        .expect("two turns have no summary");

    assert_eq!(task.turn_ids, [
        EventId::fixed("turn001"),
        EventId::fixed("turn003")
    ]);
    assert_eq!(task.batch_size, 5);
}

/// Every query on a summarized conversation reaches this check, so it must not
/// pay for setting up a provider it will not use.
#[test]
fn new_does_not_set_up_a_provider_when_every_turn_has_a_summary() {
    let mut events = ConversationStream::new_test();
    push_turn(&mut events, 1, "First question", "First answer");
    assert!(events.set_turn_summary(&EventId::fixed("turn001"), "Already summarized".to_owned()));

    let task = TurnSummaryTask::new(
        conversation_id(),
        &events,
        &summarizing_with_a_broken_provider(),
        NoticeSink::new(|_| {}),
    )
    .expect("the provider is never set up");

    assert!(task.is_none());
}

/// No turn fits a batch of zero, so there is no request worth setting up a
/// provider for.
#[test]
fn new_offers_nothing_with_a_batch_size_of_zero() {
    let mut events = ConversationStream::new_test();
    push_turn(&mut events, 1, "First question", "First answer");
    let mut config = summarizing_with_a_broken_provider();
    config.conversation.turn_summary.generate.batch_size = 0;

    let task = TurnSummaryTask::new(conversation_id(), &events, &config, NoticeSink::new(|_| {}))
        .expect("the provider is never set up");

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

/// A provider that cannot be set up is reported before the query commits to
/// waiting for the task.
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
        panic!("a provider without its API key cannot be set up");
    };

    assert!(error.to_string().contains(UNSET_KEY), "got: {error}");
}

// ---------------------------------------------------------------------------
// Running and syncing
// ---------------------------------------------------------------------------

/// The query that starts turn 2 summarizes turn 1, and the summary is stored on
/// turn 1 once the task syncs, after turn 2 was written.
#[tokio::test]
async fn a_turn_is_summarized_during_the_turn_after_it() {
    let mut workspace = Workspace::in_memory("root");
    let id = conversation(&mut workspace, |events| {
        push_turn(events, 1, "First question", "First answer");
    });

    // `jp query` starts the task from the conversation as it stands before
    // its own turn.
    let mut handler = TaskHandler::default();
    handler.spawn(task(id, &stored(&workspace, id), answering(TURN_1_SUMMARY)));

    // The query's own turn is written while the task runs.
    update(&workspace, id, |events| {
        push_turn(events, 2, "Second question", "Second answer");
    });

    handler
        .sync(&mut workspace, Duration::from_secs(10))
        .await
        .unwrap();

    assert_eq!(summaries(&workspace, id), [
        Some("Ask the first question".to_owned()),
        None
    ]);
}

/// The summary describes the turn as the model read it, and that is no longer
/// what the turn holds.
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

    handler
        .sync(&mut workspace, Duration::from_secs(10))
        .await
        .unwrap();

    assert_eq!(summaries(&workspace, id), [None]);
}

/// A summary stored after the task read the turn, by hand or by another `jp`
/// process, is kept rather than replaced.
#[tokio::test]
async fn a_summary_stored_before_the_task_syncs_is_kept() {
    let mut workspace = Workspace::in_memory("root");
    let id = conversation(&mut workspace, |events| {
        push_turn(events, 1, "First question", "First answer");
    });

    let mut handler = TaskHandler::default();
    handler.spawn(task(id, &stored(&workspace, id), answering(TURN_1_SUMMARY)));

    update(&workspace, id, |events| {
        assert!(events.set_turn_summary(&EventId::fixed("turn001"), "Written by hand".to_owned()));
    });

    handler
        .sync(&mut workspace, Duration::from_secs(10))
        .await
        .unwrap();

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

    handler
        .sync(&mut workspace, Duration::from_secs(10))
        .await
        .unwrap();

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

/// The lock is not waited for: the turns are offered again by the next query.
#[tokio::test]
async fn a_conversation_locked_elsewhere_is_left_alone() {
    let mut workspace = Workspace::in_memory("root");
    let id = conversation(&mut workspace, |events| {
        push_turn(events, 1, "First question", "First answer");
    });

    let mut handler = TaskHandler::default();
    handler.spawn(task(id, &stored(&workspace, id), answering(TURN_1_SUMMARY)));

    let held = lock(&workspace, id);
    handler
        .sync(&mut workspace, Duration::from_secs(10))
        .await
        .unwrap();
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

    handler
        .sync(&mut workspace, Duration::from_secs(10))
        .await
        .unwrap();

    assert_eq!(stored(&workspace, id), before);
}

// ---------------------------------------------------------------------------
// Generating and storing outside a task handler
// ---------------------------------------------------------------------------

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

/// A caller that already holds the lock, such as a turn about to let go of its
/// conversation, stores through it and keeps holding it.
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

// ---------------------------------------------------------------------------
// Checking a summary against the turn it describes
// ---------------------------------------------------------------------------

/// A summary generated for turn 1 of `events`, as `run` returns it.
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

    // Turns appended since, such as the query's own, leave it current.
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
