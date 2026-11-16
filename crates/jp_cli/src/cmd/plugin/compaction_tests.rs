use std::sync::Arc;

use camino_tempfile::{Utf8TempDir, tempdir};
use jp_config::AppConfig;
use jp_conversation::{
    Conversation, ConversationId, ConversationStream,
    event::{ChatRequest, ChatResponse},
};
use jp_plugin::message::{ReasoningCompaction, ToolCallsCompaction};
use jp_storage::backend::FsStorageBackend;
use jp_workspace::{LockResult, Workspace};
use pretty_assertions::assert_eq;

use super::*;

fn workspace_with_turns(turns: usize) -> (Workspace, ConversationId, Utf8TempDir) {
    let tmp = tempdir().unwrap();
    let fs = Arc::new(FsStorageBackend::new(&tmp.path().join(".jp")).unwrap());
    let id = ConversationId::try_from(
        chrono::DateTime::<chrono::Utc>::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000),
    )
    .unwrap();
    fs.write_test_conversation(&id, &Conversation::default());

    let mut ws = Workspace::in_memory(tmp.path()).with_backend(fs);
    ws.load_conversation_index();
    let handle = ws.acquire_conversation(&id).unwrap();
    ws.test_lock(handle).as_mut().update_events(|stream| {
        for _ in 0..turns {
            add_turn(stream);
        }
    });

    (ws, id, tmp)
}

fn add_turn(stream: &mut ConversationStream) {
    stream.start_turn(ChatRequest::from("A question"));
    stream
        .current_turn_mut()
        .add_chat_response(ChatResponse::message("An answer"))
        .build()
        .unwrap();
}

fn lock(ws: &Workspace, id: ConversationId) -> ConversationLock {
    let handle = ws.acquire_conversation(&id).unwrap();
    let LockResult::Acquired(lock) = ws.lock_conversation(handle, None).unwrap() else {
        panic!("conversation is unlocked");
    };

    lock
}

fn compacted(ws: &Workspace, id: ConversationId) -> Vec<(usize, usize)> {
    lock(ws, id).with_events(|events| {
        events
            .compactions()
            .map(|compaction| (compaction.from_turn, compaction.to_turn))
            .collect()
    })
}

fn turn(n: i64) -> NonZeroI64 {
    NonZeroI64::new(n).unwrap()
}

fn compaction(from: i64, to: i64) -> Compaction {
    Compaction {
        from: turn(from),
        to: turn(to),
        reasoning: None,
        tool_calls: None,
        summary: None,
    }
}

fn reasoning(from: i64, to: i64) -> Compaction {
    Compaction {
        reasoning: Some(ReasoningCompaction::default()),
        ..compaction(from, to)
    }
}

fn tool_calls(from: i64, to: i64, mode: ToolCallsMode) -> Compaction {
    Compaction {
        tool_calls: Some(ToolCallsCompaction {
            mode,
            over_bytes: None,
        }),
        ..compaction(from, to)
    }
}

#[tokio::test]
async fn compactions_are_stored_in_order_with_their_turns() {
    let (ws, id, _tmp) = workspace_with_turns(6);
    let compactions = [
        Compaction {
            tool_calls: Some(ToolCallsCompaction {
                mode: ToolCallsMode::Strip,
                over_bytes: Some(1024),
            }),
            ..reasoning(2, -2)
        },
        Compaction {
            summary: Some(SummaryCompaction::Text("We wrapped up.".to_owned())),
            ..compaction(-2, -1)
        },
    ];

    let stored = compact(&compactions, &AppConfig::new_test(), lock(&ws, id))
        .await
        .unwrap();

    assert_eq!(stored, [
        StoredCompaction {
            from: 2,
            to: 5,
            reasoning: true,
            tool_calls: Some(ToolCallsMode::Strip),
            summary: None,
        },
        StoredCompaction {
            from: 5,
            to: 6,
            reasoning: false,
            tool_calls: None,
            summary: Some(SummarySource::Text),
        },
    ]);
    assert_eq!(compacted(&ws, id), [(1, 4), (4, 5)]);
}

#[tokio::test]
async fn every_tool_calls_mode_is_stored_as_asked() {
    let (ws, id, _tmp) = workspace_with_turns(4);
    let compactions = [
        tool_calls(1, 1, ToolCallsMode::Strip),
        tool_calls(2, 2, ToolCallsMode::StripRequests),
        tool_calls(3, 3, ToolCallsMode::StripResponses),
        tool_calls(4, 4, ToolCallsMode::Omit),
    ];

    let stored = compact(&compactions, &AppConfig::new_test(), lock(&ws, id))
        .await
        .unwrap();

    assert_eq!(
        stored
            .iter()
            .map(|compaction| compaction.tool_calls)
            .collect::<Vec<_>>(),
        [
            Some(ToolCallsMode::Strip),
            Some(ToolCallsMode::StripRequests),
            Some(ToolCallsMode::StripResponses),
            Some(ToolCallsMode::Omit),
        ]
    );
}

// When a later compaction fails validation, earlier valid compactions in the
// same request are not stored.
#[tokio::test]
async fn a_compaction_with_nothing_to_do_refuses_the_whole_request() {
    let (ws, id, _tmp) = workspace_with_turns(3);

    let error = compact(
        &[reasoning(1, 2), compaction(2, 3)],
        &AppConfig::new_test(),
        lock(&ws, id),
    )
    .await
    .unwrap_err();

    assert_eq!(
        error,
        "compaction 2 has nothing to do: set `reasoning`, `tool_calls`, or `summary`"
    );
    assert_eq!(compacted(&ws, id), []);
}

#[tokio::test]
async fn a_range_past_the_end_is_refused() {
    let (ws, id, _tmp) = workspace_with_turns(3);

    let error = compact(&[reasoning(5, 6)], &AppConfig::new_test(), lock(&ws, id))
        .await
        .unwrap_err();

    assert_eq!(
        error,
        "compaction 1 (turns 5..6) selects none of the conversation's 3 turns"
    );
    assert_eq!(compacted(&ws, id), []);
}

#[tokio::test]
async fn a_range_that_ends_before_it_starts_is_refused() {
    let (ws, id, _tmp) = workspace_with_turns(3);

    let error = compact(&[reasoning(3, 1)], &AppConfig::new_test(), lock(&ws, id))
        .await
        .unwrap_err();

    assert_eq!(
        error,
        "compaction 1 (turns 3..1) selects none of the conversation's 3 turns"
    );
}

#[tokio::test]
async fn an_empty_request_is_refused() {
    let (ws, id, _tmp) = workspace_with_turns(3);

    let error = compact(&[], &AppConfig::new_test(), lock(&ws, id))
        .await
        .unwrap_err();

    assert_eq!(error, "the request names no compactions");
}

#[tokio::test]
async fn a_blank_summary_text_is_refused() {
    let (ws, id, _tmp) = workspace_with_turns(3);
    let blank = Compaction {
        summary: Some(SummaryCompaction::Text("  ".to_owned())),
        ..compaction(1, 2)
    };

    let error = compact(&[blank], &AppConfig::new_test(), lock(&ws, id))
        .await
        .unwrap_err();

    assert_eq!(
        error,
        "compaction 1: Compaction error: the summary text is empty; drop the value to generate a \
         summary instead"
    );
    assert_eq!(compacted(&ws, id), []);
}

// Overlapping verbatim summaries are rejected before persistence, with CLI
// flag suggestions omitted.
#[tokio::test]
async fn a_summary_that_would_split_an_earlier_one_is_refused() {
    let (ws, id, _tmp) = workspace_with_turns(6);
    let text = |from, to, text: &str| Compaction {
        summary: Some(SummaryCompaction::Text(text.to_owned())),
        ..compaction(from, to)
    };

    let error = compact(
        &[text(3, 6, "The end."), text(1, 4, "The start.")],
        &AppConfig::new_test(),
        lock(&ws, id),
    )
    .await
    .unwrap_err();

    assert_eq!(
        error,
        "Summary overlap: A summary cannot be nested inside or split across another one, so your \
         text for turns 1..4 would have to stand in for turns 1..6 as well."
    );
    assert_eq!(compacted(&ws, id), []);
}

#[tokio::test]
async fn an_unknown_summary_model_is_refused() {
    let (ws, id, _tmp) = workspace_with_turns(3);
    let generate = Compaction {
        summary: Some(SummaryCompaction::Generate(GenerateSummary {
            model: Some("no-such-model".to_owned()),
            ..GenerateSummary::default()
        })),
        ..compaction(1, 2)
    };

    let error = compact(&[generate], &AppConfig::new_test(), lock(&ws, id))
        .await
        .unwrap_err();

    assert_eq!(
        error,
        "compaction 1: unknown model `no-such-model`: model ID does not match a known alias nor \
         matches the full ID format <provider>/<model>"
    );
}

#[test]
fn a_named_summary_model_keeps_the_conversations_model_settings() {
    let config = AppConfig::new_test();
    let generate = Compaction {
        summary: Some(SummaryCompaction::Generate(GenerateSummary {
            model: Some("anthropic/claude-haiku-5-5".to_owned()),
            context: Some("the decisions".to_owned()),
            ..GenerateSummary::default()
        })),
        ..compaction(1, 2)
    };

    let rule = rule(1, &generate, &config).unwrap();

    let summary = rule.summary.unwrap();
    let model = summary.model.unwrap();
    assert_eq!(model.id.to_string(), "anthropic/claude-haiku-5-5");
    assert_eq!(model.parameters, config.assistant.model.parameters);
    assert_eq!(summary.context.as_deref(), Some("the decisions"));
    assert_eq!(summary.text, None);
}

#[test]
fn a_turn_counts_from_the_first_or_from_the_last() {
    assert_eq!(bound(turn(1)), RuleBound::Absolute(1));
    assert_eq!(bound(turn(7)), RuleBound::Absolute(7));
    assert_eq!(bound(turn(-1)), RuleBound::FromEnd(0));
    assert_eq!(bound(turn(-3)), RuleBound::FromEnd(2));
}
