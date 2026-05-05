//! Id-keyed scratchpad for tool work, plus the typed execution plan.
//!
//! `PendingTools` caches the per-tool work product produced during the
//! streaming phase: either a prepared executor (permission approved) or a
//! pre-resolved [`ToolCallResponse`] (permission denied / tool unavailable).
//!
//! The shape is deliberately restrictive:
//!
//! - The only way to retrieve an entry is [`PendingTools::take`] by id.
//! - The only ids that exist come from walking the conversation stream.
//! - There is no `iter()`, `into_vec()`, or `drain()`.
//!
//! Combined with [`build_execution_plan`] — the **single** constructor for
//! [`ExecutionPlan`] — this makes "the conversation stream is the source of
//! truth for tool execution" hold at the API level, not by convention. A
//! future contributor who wants a bag-of-pending-work has to either walk
//! the stream first or add a new public API method, which is the visible
//! smell that earlier conventions lacked.
//!
//! See also: the `JP refactor` Bear note for the prep-flow unification
//! follow-up.
//!
//! [`ToolCallResponse`]: jp_conversation::event::ToolCallResponse
use std::collections::HashMap;

use jp_conversation::{
    ConversationStream,
    event::{ToolCallRequest, ToolCallResponse},
};
use jp_llm::tool::executor::Executor;

/// The work product for a single tool call, as decided during the streaming
/// phase.
pub(crate) enum PendingEntry {
    /// Permission was approved and the executor is ready to run.
    Approved(Box<dyn Executor>),
    /// Permission was denied (`Skip`) or the tool couldn't be resolved
    /// (`Unavailable`); the response is already determined and just needs
    /// to be committed in the right order.
    Resolved(ToolCallResponse),
}

/// Id-keyed scratchpad for the streaming phase's tool-prep output.
///
/// Insert during streaming (`insert_approved` / `insert_resolved`), retrieve
/// per-id during the executing phase via [`PendingTools::take`]. There is no
/// way to enumerate the contents — callers must derive ids from the
/// conversation stream.
#[derive(Default)]
pub(crate) struct PendingTools {
    entries: HashMap<String, PendingEntry>,
}

impl PendingTools {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Record an approved executor for `id`.
    pub(crate) fn insert_approved(&mut self, id: String, executor: Box<dyn Executor>) {
        self.entries.insert(id, PendingEntry::Approved(executor));
    }

    /// Record a pre-resolved response (skipped or unavailable) for `id`.
    pub(crate) fn insert_resolved(&mut self, id: String, response: ToolCallResponse) {
        self.entries.insert(id, PendingEntry::Resolved(response));
    }

    /// Take the entry for `id`, if any. There is no way to retrieve entries
    /// other than by id — and the only place ids come from is the
    /// conversation stream.
    pub(crate) fn take(&mut self, id: &str) -> Option<PendingEntry> {
        self.entries.remove(id)
    }

    /// Number of entries currently held. Provided for diagnostics and
    /// tests only; production code SHOULD NOT use this to infer execution
    /// work — that's what [`build_execution_plan`] is for.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

/// One ordered work item in an [`ExecutionPlan`].
///
/// `index` is the tool's position among unresponded tool-call requests in the
/// current turn, in document order. It matches the `perm_tool_index` numbering
/// of the previous design and is used downstream to merge response Vecs in
/// the right order.
pub(crate) struct PlanItem {
    pub(crate) index: usize,
    /// The original request, kept for debugging and tests. Production code
    /// drives execution off `index` + `work`; the request id is already
    /// inside `work` for both `Approved` (`executor.tool_id()`) and
    /// `Resolved` (`response.id`).
    #[allow(dead_code)]
    pub(crate) request: ToolCallRequest,
    pub(crate) work: PendingEntry,
}

/// Ordered tool work for the current cycle's executing phase, derived from
/// the conversation stream.
///
/// The only public constructor is [`build_execution_plan`]. A future
/// contributor who wants to skip the stream walk can't fabricate one of
/// these without also adding a new constructor — which is the visible
/// smell.
pub(crate) struct ExecutionPlan {
    items: Vec<PlanItem>,

    /// Tool-call requests that appear in the stream's current turn without a
    /// matching response AND without a matching entry in `PendingTools`.
    /// Should be empty in correct operation; a non-empty vec signals a
    /// contract violation (some path added a `ToolCallRequest` to the stream
    /// without going through the streaming-phase preparation flow). The
    /// caller decides what to do — synthesize an error response, log, etc.
    orphaned: Vec<ToolCallRequest>,
}

impl ExecutionPlan {
    /// Decompose the plan into its parts. Consumes the plan; there's no
    /// way to read it twice.
    pub(crate) fn into_parts(self) -> (Vec<PlanItem>, Vec<ToolCallRequest>) {
        (self.items, self.orphaned)
    }

    /// `true` when there's no work and no orphans.
    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty() && self.orphaned.is_empty()
    }
}

/// Build an [`ExecutionPlan`] by walking the current turn for unresponded
/// `ToolCallRequest`s and matching them against `pending`.
///
/// This is the single entry point for "what work do we need to do this
/// cycle?" Other code MUST NOT bypass this function — there's no other way
/// to construct an `ExecutionPlan`.
pub(crate) fn build_execution_plan(
    stream: &ConversationStream,
    pending: &mut PendingTools,
) -> ExecutionPlan {
    // Collect ids that already have a response anywhere in the stream.
    // This intentionally looks across all turns, not just the current one,
    // so a response committed earlier (e.g. during a prior cycle of the
    // same turn) correctly suppresses the request from being re-executed.
    let responded_ids: std::collections::HashSet<&str> = stream
        .iter()
        .filter_map(|e| e.event.as_tool_call_response())
        .map(|r| r.id.as_str())
        .collect();

    // Walk the most recent turn for ToolCallRequests, in document order.
    let Some(current_turn) = stream.iter_turns().next_back() else {
        return ExecutionPlan {
            items: Vec::new(),
            orphaned: Vec::new(),
        };
    };

    let mut items = Vec::new();
    let mut orphaned = Vec::new();

    for event in &current_turn {
        let Some(request) = event.event.as_tool_call_request() else {
            continue;
        };
        if responded_ids.contains(request.id.as_str()) {
            continue;
        }

        let index = items.len() + orphaned.len();
        match pending.take(&request.id) {
            Some(work) => items.push(PlanItem {
                index,
                request: request.clone(),
                work,
            }),
            None => orphaned.push(request.clone()),
        }
    }

    ExecutionPlan { items, orphaned }
}

#[cfg(test)]
#[path = "pending_tests.rs"]
mod tests;
