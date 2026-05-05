use jp_conversation::{
    ConversationStream,
    event::{ChatRequest, ChatResponse, ToolCallRequest, ToolCallResponse},
};
use jp_llm::tool::executor::MockExecutor;
use serde_json::Map;

use super::*;

fn req(id: &str, name: &str) -> ToolCallRequest {
    ToolCallRequest {
        id: id.into(),
        name: name.into(),
        arguments: Map::new(),
    }
}

fn resp(id: &str, content: &str) -> ToolCallResponse {
    ToolCallResponse {
        id: id.into(),
        result: Ok(content.into()),
    }
}

fn approved_executor(id: &str, name: &str) -> Box<dyn Executor> {
    Box::new(MockExecutor::completed(id, name, "done"))
}

#[test]
fn empty_stream_yields_empty_plan() {
    let stream = ConversationStream::new_test();
    let mut pending = PendingTools::new();

    let plan = build_execution_plan(&stream, &mut pending);
    assert!(plan.is_empty());
}

/// Plan items appear in document order, indexed 0..N. This matches the
/// `perm_tool_index` numbering of the previous design and what
/// `commit_tool_responses` expects when merging Vecs by index.
#[test]
fn plan_items_are_in_document_order() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn(ChatRequest::from("user"));
    stream
        .current_turn_mut()
        .add_tool_call_request(req("a", "tool_a"))
        .add_tool_call_request(req("b", "tool_b"))
        .add_tool_call_request(req("c", "tool_c"))
        .build()
        .unwrap();

    let mut pending = PendingTools::new();
    pending.insert_approved("a".into(), approved_executor("a", "tool_a"));
    pending.insert_resolved("b".into(), resp("b", "skipped"));
    pending.insert_approved("c".into(), approved_executor("c", "tool_c"));

    let plan = build_execution_plan(&stream, &mut pending);
    let (items, orphaned) = plan.into_parts();

    assert!(orphaned.is_empty());
    assert_eq!(items.len(), 3);
    assert_eq!(items[0].request.id, "a");
    assert_eq!(items[0].index, 0);
    assert_eq!(items[1].request.id, "b");
    assert_eq!(items[1].index, 1);
    assert_eq!(items[2].request.id, "c");
    assert_eq!(items[2].index, 2);
    assert!(matches!(items[0].work, PendingEntry::Approved(_)));
    assert!(matches!(items[1].work, PendingEntry::Resolved(_)));
    assert!(matches!(items[2].work, PendingEntry::Approved(_)));
}

/// Already-responded requests are skipped — this is what makes the
/// stream-as-source-of-truth design work across multiple cycles within a
/// single turn. Cycle 1's requests have responses by cycle 2; only cycle
/// 2's new unresponded requests should appear in the plan.
#[test]
fn responded_requests_are_skipped() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn(ChatRequest::from("user"));
    stream
        .current_turn_mut()
        .add_tool_call_request(req("done", "tool"))
        .add_tool_call_response(resp("done", "ok"))
        .add_tool_call_request(req("pending", "tool"))
        .build()
        .unwrap();

    let mut pending = PendingTools::new();
    pending.insert_approved("pending".into(), approved_executor("pending", "tool"));

    let plan = build_execution_plan(&stream, &mut pending);
    let (items, orphaned) = plan.into_parts();

    assert!(orphaned.is_empty());
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].request.id, "pending");
    assert_eq!(items[0].index, 0);
}

/// A request in the stream without a matching pending entry is reported as
/// orphaned. In normal operation this should never happen — the streaming
/// phase always populates pending for every request it adds. Treating it
/// as a hard error gives us early signal if a future code path bypasses
/// the prep flow.
#[test]
fn unmatched_request_is_reported_as_orphaned() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn(ChatRequest::from("user"));
    stream
        .current_turn_mut()
        .add_tool_call_request(req("ghost", "tool"))
        .build()
        .unwrap();

    let mut pending = PendingTools::new();

    let plan = build_execution_plan(&stream, &mut pending);
    let (items, orphaned) = plan.into_parts();

    assert!(items.is_empty());
    assert_eq!(orphaned.len(), 1);
    assert_eq!(orphaned[0].id, "ghost");
}

/// The plan walks only the current (most recent) turn. Tool calls from a
/// prior turn — even unresponded ones — must not leak in. The top-level
/// `query.rs` sanitize pass and the per-cycle sanitize in `run_turn_loop`
/// handle prior-turn orphans separately.
#[test]
fn previous_turn_requests_are_ignored() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn(ChatRequest::from("first"));
    stream
        .current_turn_mut()
        .add_tool_call_request(req("old_unresponded", "tool"))
        .build()
        .unwrap();
    stream.start_turn(ChatRequest::from("second"));
    stream
        .current_turn_mut()
        .add_tool_call_request(req("new", "tool"))
        .build()
        .unwrap();

    let mut pending = PendingTools::new();
    pending.insert_approved("new".into(), approved_executor("new", "tool"));
    // Note: no entry for old_unresponded — it shouldn't matter; we don't
    // walk into the previous turn.

    let plan = build_execution_plan(&stream, &mut pending);
    let (items, orphaned) = plan.into_parts();

    assert!(orphaned.is_empty());
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].request.id, "new");
}

/// `take` returns each entry exactly once. The signature reflects the
/// invariant: an `ExecutionPlan` consumes its corresponding pending
/// entries, leaving the cache empty for the next cycle.
#[test]
fn build_consumes_pending_entries() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn(ChatRequest::from("user"));
    stream
        .current_turn_mut()
        .add_tool_call_request(req("only", "tool"))
        .build()
        .unwrap();

    let mut pending = PendingTools::new();
    pending.insert_approved("only".into(), approved_executor("only", "tool"));
    assert_eq!(pending.len(), 1);

    let _plan = build_execution_plan(&stream, &mut pending);

    assert_eq!(pending.len(), 0, "build must drain matched entries");
}

/// Defensive: if some path injects a `ChatResponse` (or any other non-tool
/// event) into the current turn, it doesn't break the plan walk.
#[test]
fn non_tool_events_are_ignored() {
    let mut stream = ConversationStream::new_test();
    stream.start_turn(ChatRequest::from("user"));
    stream
        .current_turn_mut()
        .add_chat_response(ChatResponse::message("thinking"))
        .add_tool_call_request(req("a", "tool_a"))
        .add_chat_response(ChatResponse::message("more text"))
        .add_tool_call_request(req("b", "tool_b"))
        .build()
        .unwrap();

    let mut pending = PendingTools::new();
    pending.insert_approved("a".into(), approved_executor("a", "tool_a"));
    pending.insert_approved("b".into(), approved_executor("b", "tool_b"));

    let plan = build_execution_plan(&stream, &mut pending);
    let (items, orphaned) = plan.into_parts();

    assert!(orphaned.is_empty());
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].request.id, "a");
    assert_eq!(items[0].index, 0);
    assert_eq!(items[1].request.id, "b");
    assert_eq!(items[1].index, 1);
}
