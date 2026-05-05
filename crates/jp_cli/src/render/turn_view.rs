//! Shared role-aware rendering for both replay and live streaming.
//!
//! [`TurnView`] coordinates the chat and structured sub-renderers and
//! tracks turn-level state — most importantly, whether the assistant role
//! header has been emitted yet.
//!
//! Both [`TurnRenderer`] (replay, e.g. `jp conversation print`) and
//! `TurnCoordinator` (live, the streaming-query pipeline) own a `TurnView` and
//! route their rendering through it. This keeps role attribution, content-kind
//! transitions, and structured-output dispatch consistent across the two flows
//! so changes to one don't silently drift from the other.
//!
//! [`TurnRenderer`]: super::TurnRenderer

use std::sync::Arc;

use jp_config::style::StyleConfig;
use jp_conversation::event::{ChatRequest, ChatResponse};
use jp_printer::Printer;

use super::{ChatRenderer, StructuredRenderer};

/// Fallback label used when no [`assistant.name`][an] is configured.
///
/// [an]: jp_config::assistant::AssistantConfig::name
pub(crate) const DEFAULT_ASSISTANT_LABEL: &str = "jp";

/// Fallback label used when a [`ChatRequest`] has no [`author`][a]
/// stamped on it (typically because no [`user.name`][un] was configured at
/// event-creation time).
///
/// [a]: ChatRequest::author
/// [un]: jp_config::user::UserConfig::name
pub(crate) const DEFAULT_USER_LABEL: &str = "user";

/// Coordinates role-aware rendering for a single conversation turn.
///
/// Owns the chat and structured sub-renderers and tracks whether the
/// assistant role header has been emitted for the current turn.
pub(crate) struct TurnView {
    chat: ChatRenderer,
    structured: StructuredRenderer,

    assistant_name: Option<String>,
    model_id: Option<String>,

    /// Whether the assistant role header has been emitted for the current
    /// turn. Reset by [`Self::begin_turn`] and [`Self::render_user_request`];
    /// set by [`Self::ensure_assistant_header`] on first use.
    assistant_header_rendered: bool,
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
        }
    }

    /// Mark the start of a new turn. The next assistant event will emit a
    /// fresh role header.
    pub fn begin_turn(&mut self) {
        self.assistant_header_rendered = false;
    }

    /// Render a user request: a labeled role header followed by the request
    /// body. Resets assistant-header gating so the next assistant event
    /// emits a fresh header.
    pub fn render_user_request(&mut self, req: &ChatRequest) {
        let label = req.author.as_deref().unwrap_or(DEFAULT_USER_LABEL);
        self.chat.render_role_header(label, None);
        self.chat.render_request(&req.content);
        self.assistant_header_rendered = false;
    }

    /// Render a chat response chunk (or full event), emitting the assistant
    /// role header first if it hasn't been emitted yet for this turn.
    ///
    /// Dispatches structured responses to the structured renderer and
    /// everything else (messages, reasoning) to the chat renderer.
    pub fn render_chat_response(&mut self, resp: &ChatResponse) {
        self.ensure_assistant_header();
        if resp.is_structured() {
            self.chat.flush();
            self.structured.render_chunk(resp);
        } else {
            self.chat.render_response(resp);
        }
    }

    /// Mark a tool call boundary in the chat renderer.
    ///
    /// Emits the assistant header if not already shown, then flushes the
    /// chat buffer so surrounding messages render as distinct paragraphs.
    /// `hidden` controls whether the chat renderer transitions into the
    /// `ToolCall` content kind: passing `true` keeps the boundary
    /// invisible (suitable for hidden tool calls so the next message
    /// doesn't pick up an extra blank line); `false` is the normal case
    /// where tool UI follows.
    pub fn enter_tool_call(&mut self, hidden: bool) {
        self.ensure_assistant_header();
        self.chat.flush();
        if !hidden {
            self.chat.transition_to_tool_call();
        }
    }

    /// Flush pending chat output.
    pub fn flush(&mut self) {
        self.chat.flush();
    }

    /// Flush pending output for both chat and structured renderers.
    ///
    /// Used at the end of a streaming cycle so any unclosed JSON code
    /// fence gets its closing fence before the next phase starts.
    pub fn flush_all(&mut self) {
        self.chat.flush();
        self.structured.flush();
    }

    /// Reset internal renderer state, discarding partial buffers.
    ///
    /// Used when a streaming cycle is interrupted and a new one begins
    /// (e.g. interrupt-with-prefill). Crucially, this leaves the
    /// assistant-header flag set: the continuation is part of the same
    /// turn, so re-emitting the header would be a duplicate.
    pub fn reset_for_continuation(&mut self) {
        self.chat.reset();
        self.structured.reset();
        self.assistant_header_rendered = true;
    }

    /// Replace the underlying renderers and identity.
    ///
    /// Used by replay's per-turn config rebuild when the conversation's
    /// historical config differs from the workspace's current config.
    /// Header gating state is preserved (the rebuild itself doesn't open
    /// a new turn).
    pub fn reconfigure(
        &mut self,
        printer: Arc<Printer>,
        style: StyleConfig,
        assistant_name: Option<String>,
        model_id: Option<String>,
    ) {
        self.chat = ChatRenderer::new(printer.clone(), style);
        self.structured = StructuredRenderer::new(printer);
        self.assistant_name = assistant_name;
        self.model_id = model_id;
    }

    fn ensure_assistant_header(&mut self) {
        if self.assistant_header_rendered {
            return;
        }
        let label = self
            .assistant_name
            .as_deref()
            .unwrap_or(DEFAULT_ASSISTANT_LABEL);
        self.chat
            .render_role_header(label, self.model_id.as_deref());
        self.assistant_header_rendered = true;
    }
}
