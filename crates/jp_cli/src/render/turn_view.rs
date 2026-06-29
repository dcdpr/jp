//! Shared role-aware rendering for both replay and live streaming.
//!
//! [`TurnView`] coordinates the chat and structured sub-renderers and tracks
//! turn-level state — most importantly, whether the assistant role header has
//! been emitted yet.
//!
//! Both [`TurnRenderer`] (replay, e.g. `jp conversation print`) and
//! `TurnCoordinator` (live, the streaming-query pipeline) own a `TurnView` and
//! route their rendering through it.
//! This keeps role attribution, content-kind transitions, and structured-output
//! dispatch consistent across the two flows so changes to one don't silently
//! drift from the other.
//!
//! [`TurnRenderer`]: super::TurnRenderer

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use jp_config::style::StyleConfig;
use jp_conversation::event::{ChatRequest, ChatResponse};
use jp_printer::Printer;

use super::{ChatRenderer, StructuredRenderer};

/// Fallback label used when no [`assistant.name`][an] is configured.
///
/// [an]: jp_config::assistant::AssistantConfig::name
pub(crate) const DEFAULT_ASSISTANT_LABEL: &str = "jp";

/// Fallback label used when a [`ChatRequest`] has no [`author`][a] stamped on
/// it (typically because no [`user.name`][un] was configured at event-creation
/// time).
///
/// [a]: ChatRequest::author
/// [un]: jp_config::user::UserConfig::name
pub(crate) const DEFAULT_USER_LABEL: &str = "user";

/// Coordinates role-aware rendering for a single conversation turn.
///
/// Owns the chat and structured sub-renderers and tracks whether the assistant
/// role header has been emitted for the current turn.
pub(crate) struct TurnView {
    chat: ChatRenderer,
    structured: StructuredRenderer,

    assistant_name: Option<String>,
    model_id: Option<String>,

    /// Whether the assistant role header has been emitted for the current turn.
    /// Reset by [`Self::begin_turn`] and [`Self::render_user_request`]; set by
    /// [`Self::ensure_assistant_header`] on first use.
    assistant_header_rendered: bool,

    /// Dimmed detail (e.g.
    /// `turn 2, 12 minutes ago`) to append to the first role header rendered in
    /// the current turn.
    /// Consumed by [`Self::emit_role_header`], which every header path routes
    /// through, so the first header in a turn takes it and later ones don't.
    /// Set per turn by replay via [`Self::set_turn_detail`]; left `None` by the
    /// live query path.
    pending_turn_detail: Option<String>,

    /// Shared with the [`ToolRenderer`] (wired via
    /// [`Self::set_tool_separator`]): the flag a tool result or custom argument
    /// block raises to owe a blank-line separator before the next tool call.
    /// Visible assistant content clears it, since it supplies its own spacing.
    ///
    /// [`ToolRenderer`]: super::ToolRenderer
    tool_separator: Arc<AtomicBool>,
}

impl TurnView {
    pub fn new(
        printer: Arc<Printer>,
        style: StyleConfig,
        assistant_name: Option<String>,
        model_id: Option<String>,
    ) -> Self {
        Self {
            chat: ChatRenderer::new(printer.clone(), style),
            structured: StructuredRenderer::new(printer),
            assistant_name,
            model_id,
            assistant_header_rendered: false,
            pending_turn_detail: None,
            tool_separator: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Wire this view's tool-separator flag to a [`ToolRenderer`].
    ///
    /// [`ToolRenderer`]: super::ToolRenderer
    pub(crate) fn set_tool_separator(&mut self, flag: Arc<AtomicBool>) {
        self.tool_separator = flag;
    }

    /// Set the dimmed detail attached to the first role header of the upcoming
    /// turn (e.g.
    /// `turn 2, 12 minutes ago`).
    ///
    /// Consumed by whichever header — user or assistant — renders first;
    /// later headers in the same turn render without it.
    pub(crate) fn set_turn_detail(&mut self, detail: Option<String>) {
        self.pending_turn_detail = detail;
    }

    /// Mark the start of a new turn.
    /// The next assistant event will emit a fresh role header.
    /// Closes any open structured fence so a turn that ended on a
    /// `ChatResponse::Structured` doesn't bleed into the next turn's content.
    pub fn begin_turn(&mut self) {
        self.structured.flush();
        self.assistant_header_rendered = false;
    }

    /// Render a user request: a labeled role header followed by the request
    /// body.
    /// Resets assistant-header gating so the next assistant event emits a fresh
    /// header.
    pub fn render_user_request(&mut self, req: &ChatRequest) {
        // Close any open structured fence before the user header so the
        // boundary marker isn't rendered inside a `json` block.
        self.structured.flush();
        self.tool_separator.store(false, Ordering::Relaxed);
        let label = req.author.as_deref().unwrap_or(DEFAULT_USER_LABEL);
        self.emit_role_header(label, None);
        self.chat.render_request(&req.content);
        self.assistant_header_rendered = false;
    }

    /// Render a chat response chunk (or full event), emitting the assistant
    /// role header first if it hasn't been emitted yet for this turn.
    ///
    /// Dispatches structured responses to the structured renderer and
    /// everything else (messages, reasoning) to the chat renderer.
    /// A non-structured response after structured content closes the open
    /// `json` fence first; a structured response after non-structured content
    /// flushes the chat buffer first.
    pub fn render_chat_response(&mut self, resp: &ChatResponse) {
        self.ensure_assistant_header();

        // Visible assistant content supplies its own spacing, so a preceding
        // tool block no longer owes a separator before the next tool call.
        // Reasoning that doesn't supply its own separation (Hidden renders
        // nothing; Timer erases its line; Progress leaves an unterminated
        // `reasoning...` line) must not clear that debt.
        let clears_debt = match resp {
            ChatResponse::Reasoning { .. } => self.chat.reasoning_supplies_separation(),
            _ => true,
        };
        if clears_debt {
            self.tool_separator.store(false, Ordering::Relaxed);
        }

        if resp.is_structured() {
            self.chat.flush();
            self.structured.render_chunk(resp);
        } else {
            self.structured.flush();
            self.chat.render_response(resp);
        }
    }

    /// Mark a tool call boundary in the chat renderer.
    ///
    /// Emits the assistant header if not already shown, then flushes the chat
    /// buffer so surrounding messages render as distinct paragraphs.
    /// Also closes any open structured fence — a tool call after structured
    /// output is a content boundary that must not stay inside the `json` block.
    ///
    /// `hidden` controls whether the chat renderer transitions into the
    /// `ToolCall` content kind: passing `true` keeps the boundary invisible
    /// (suitable for hidden tool calls so the next message doesn't pick up an
    /// extra blank line); `false` is the normal case where tool UI follows.
    pub fn enter_tool_call(&mut self, hidden: bool) {
        self.ensure_assistant_header();
        self.structured.flush();
        self.chat.flush();
        if !hidden {
            self.chat.transition_to_tool_call();
        }
    }

    /// Flush pending output across both chat and structured renderers.
    ///
    /// Closes any open `json` fence and drains any buffered chat content.
    /// Safe to call at any boundary; in particular, replay's final flush after
    /// the last turn relies on this to terminate a trailing structured
    /// response.
    pub fn flush(&mut self) {
        self.chat.flush();
        self.structured.flush();
    }

    /// Signal to the printer that the current streaming cycle has ended.
    ///
    /// Forwards to the chat renderer, which switches the printer's
    /// bounded-latency controller into drain mode so its per-character delay
    /// holds its current pace as the queue empties (instead of slowing back
    /// toward the configured cap).
    pub fn signal_typewriter_drain(&self) {
        self.chat.signal_typewriter_drain();
    }

    /// Reset internal renderer state, discarding partial buffers.
    ///
    /// Used when a streaming cycle is interrupted and a new one begins (e.g.
    /// interrupt-with-prefill).
    /// Preserves the existing `assistant_header_rendered` flag: if a header was
    /// already on the terminal before the interrupt, the continuation is part
    /// of the same assistant turn and must not re-emit it; if no header had
    /// been rendered yet (the user interrupted before the first chunk), the
    /// flag stays `false` and the next assistant event will emit one.
    pub fn reset_for_continuation(&mut self) {
        self.chat.reset();
        self.structured.reset();
    }

    /// Replace the underlying renderers and identity.
    ///
    /// Used by replay's per-turn config rebuild when the conversation's
    /// historical config differs from the workspace's current config.
    /// Header gating state is preserved (the rebuild itself doesn't open a new
    /// turn).
    /// Flushes both sub-renderers before swapping them out so any open `json`
    /// fence or buffered chat output is committed before the new instances take
    /// over.
    pub fn reconfigure(
        &mut self,
        printer: Arc<Printer>,
        style: StyleConfig,
        assistant_name: Option<String>,
        model_id: Option<String>,
    ) {
        self.flush();
        self.chat = ChatRenderer::new(printer.clone(), style);
        self.structured = StructuredRenderer::new(printer);
        self.assistant_name = assistant_name;
        self.model_id = model_id;
    }

    /// Emit a role-boundary header, attaching the pending turn detail to it.
    ///
    /// The single place that consumes `pending_turn_detail`: the first header
    /// rendered in a turn takes the detail, every later header renders without
    /// it.
    /// Both the user and assistant header paths route through here so the
    /// "first header wins" rule lives in one spot instead of being
    /// re-implemented at each call site.
    fn emit_role_header(&mut self, label: &str, suffix: Option<&str>) {
        let detail = self.pending_turn_detail.take();
        self.chat
            .render_role_header(label, suffix, detail.as_deref());
    }

    fn ensure_assistant_header(&mut self) {
        if self.assistant_header_rendered {
            return;
        }
        // Cloned into owned locals so the `&mut self` call to `emit_role_header`
        // doesn't overlap with shared borrows of these fields.
        let label = self
            .assistant_name
            .clone()
            .unwrap_or_else(|| DEFAULT_ASSISTANT_LABEL.to_owned());
        let suffix = self.model_id.clone();
        self.emit_role_header(&label, suffix.as_deref());
        self.assistant_header_rendered = true;
    }
}

#[cfg(test)]
#[path = "turn_view_tests.rs"]
mod tests;
