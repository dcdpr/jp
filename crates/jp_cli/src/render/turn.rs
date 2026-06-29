//! Turn-level rendering for conversation replay.
//!
//! The [`TurnRenderer`] coordinates the [`TurnView`] (which owns the chat and
//! structured sub-renderers) and the [`ToolRenderer`] to render a complete
//! conversation event stream.
//! It handles turn boundaries, per-turn config rebuilds, and tool config
//! lookups.

use std::{collections::HashMap, sync::Arc};

use camino::Utf8PathBuf;
use chrono::Utc;
use jp_config::{
    AppConfig, PartialAppConfig,
    conversation::tool::{ToolConfigWithDefaults, ToolsConfig, style::ParametersStyle},
    model::id::PartialModelIdOrAliasConfig,
    style::{StyleConfig, typewriter::DelayDuration},
};
use jp_conversation::{EventKind, stream::turn_iter::Turn};
use jp_llm::tool::InvocationContext;
use jp_printer::{ErrChannel, Printer};
use tracing::warn;

use super::{ToolRenderer, TurnView, metadata::get_rendered_arguments};

/// Controls where the renderer sources its configuration from.
#[derive(Debug, Clone)]
pub enum ConfigSource {
    /// Use the config as it was when each turn was created.
    ///
    /// The renderer rebuilds its sub-renderers at each turn boundary using the
    /// accumulated `PartialAppConfig` from the event stream.
    PerTurn,

    /// Use a fixed config for all turns (the current workspace config).
    Fixed,
}

/// Renders conversation events for replay (e.g.
/// `jp conversation print`).
///
/// Owns a [`TurnView`] for chat/structured rendering and a [`ToolRenderer`] for
/// tool UI; dispatches each event to the right one and rebuilds the view at
/// turn boundaries when in [`ConfigSource::PerTurn`] mode.
pub struct TurnRenderer {
    // Stable params for rebuilding sub-renderers.
    printer: Arc<Printer>,
    root: Utf8PathBuf,
    is_tty: bool,
    source: ConfigSource,
    invocation: InvocationContext,

    view: TurnView,
    tool: ToolRenderer,
    tools_config: ToolsConfig,

    /// Maps tool call IDs to tool names, populated as `ToolCallRequest` events
    /// are encountered so that `ToolCallResponse` can look up the name without
    /// needing access to the full conversation stream.
    tool_names: HashMap<String, String>,
}

impl TurnRenderer {
    pub fn new(
        printer: Arc<Printer>,
        style: StyleConfig,
        tools_config: ToolsConfig,
        assistant_name: Option<String>,
        model_id: Option<String>,
        root: Utf8PathBuf,
        is_tty: bool,
        source: ConfigSource,
        invocation: InvocationContext,
    ) -> Self {
        let mut view = TurnView::new(printer.clone(), style.clone(), assistant_name, model_id);
        let tool = ToolRenderer::new(
            ErrChannel::new(printer.clone()),
            style,
            root.clone(),
            is_tty,
            invocation.clone(),
        );
        view.set_tool_separator(tool.separator_flag());
        Self {
            printer,
            root,
            is_tty,
            source,
            invocation,
            view,
            tool,
            tools_config,
            tool_names: HashMap::new(),
        }
    }

    /// Render all events in a turn.
    pub fn render_turn(&mut self, turn: &Turn<'_>) {
        if matches!(self.source, ConfigSource::PerTurn)
            && let Some(partial) = turn.iter().next().map(|e| &e.config)
        {
            self.reconfigure(partial);
        }

        self.view.set_turn_detail(turn_detail(turn));

        for event_with_cfg in turn {
            match &event_with_cfg.event.kind {
                EventKind::TurnStart(_) => {
                    self.view.begin_turn();
                }

                EventKind::ChatRequest(req) => {
                    self.view.render_user_request(req);
                }

                EventKind::ChatResponse(resp) => {
                    self.view.render_chat_response(resp);
                }

                EventKind::ToolCallRequest(req) => {
                    self.tool_names.insert(req.id.clone(), req.name.clone());

                    let default_style = &self.tools_config.defaults.style;
                    let tool_cfg = self.tools_config.get(&req.name);
                    let style = tool_cfg
                        .as_ref()
                        .map_or(default_style, ToolConfigWithDefaults::style);

                    self.view.enter_tool_call(style.hidden);

                    if !style.hidden {
                        self.tool
                            .render_tool_call(&req.name, &req.arguments, &style.parameters);

                        // Show stored custom-formatter output when replaying
                        // a tool call that was originally rendered with a
                        // Custom parameters style.
                        if matches!(style.parameters, ParametersStyle::Custom(_))
                            && let Some(rendered) = get_rendered_arguments(event_with_cfg.event)
                        {
                            self.tool.render_formatted_arguments(&rendered);
                        }
                    }
                }

                EventKind::ToolCallResponse(resp) => {
                    let name = self.tool_names.get(&resp.id);
                    let default_style = &self.tools_config.defaults.style;
                    let tool_cfg = name.and_then(|n| self.tools_config.get(n));
                    let tool_style = tool_cfg.as_ref().map_or(default_style, |c| c.style());
                    let is_error = resp.result.is_err();
                    let hidden = tool_style.hidden;
                    let inline_results = tool_style.inline_results(is_error);
                    let results_file_link = tool_style.results_file_link(is_error);

                    if !hidden {
                        self.tool
                            .render_result(resp, inline_results, results_file_link);
                    }
                }

                EventKind::InquiryRequest(_) | EventKind::InquiryResponse(_) => {}
            }
        }
    }

    /// Flush all sub-renderers.
    /// Call after the last turn has been rendered.
    pub fn flush(&mut self) {
        self.view.flush();
    }

    /// Rebuild sub-renderers from a per-turn config partial.
    fn reconfigure(&mut self, partial: &PartialAppConfig) {
        let assistant_name = partial.assistant.name.clone();
        let model_id = render_model_id(&partial.assistant.model.id);

        let config = match AppConfig::from_partial_with_defaults(partial.clone()) {
            Ok(config) => config,
            Err(err) => {
                warn!(%err, "Failed to build per-turn config, keeping current config.");
                return;
            }
        };

        let mut style = config.style;
        style.typewriter.text_delay = DelayDuration::instant();
        style.typewriter.code_delay = DelayDuration::instant();

        self.view.reconfigure(
            self.printer.clone(),
            style.clone(),
            assistant_name,
            model_id,
        );
        self.tool = ToolRenderer::new(
            ErrChannel::new(self.printer.clone()),
            style,
            self.root.clone(),
            self.is_tty,
            self.invocation.clone(),
        );
        self.view.set_tool_separator(self.tool.separator_flag());
        self.tools_config = config.conversation.tools;
    }
}

/// Build the dimmed right-aligned header detail for a turn: its 1-based number
/// and how long ago it started, e.g. `turn 2, 12 minutes ago`.
///
/// The timestamp comes from the turn's `TurnStart` marker, falling back to the
/// turn's first event when the marker is absent (the implicit leading turn of a
/// legacy stream).
fn turn_detail(turn: &Turn<'_>) -> Option<String> {
    let started_at = turn
        .iter()
        .find(|e| e.event.is_turn_start())
        .or_else(|| turn.iter().next())
        .map(|e| e.event.timestamp)?;

    // A negative duration (a `TurnStart` ahead of now, from clock skew or
    // imported data) fails `to_std` and falls back to zero, which `timeago`
    // renders as "now" rather than a misleading "... ago".
    let elapsed = (Utc::now() - started_at).to_std().unwrap_or_default();
    let ago = timeago::Formatter::new().convert(elapsed);
    Some(format!("turn {}, {ago}", turn.index() + 1))
}

/// Render a partial model id as a display string, treating a fully-empty id as
/// "no model" rather than the empty string.
///
/// The partial's `Display` impl already handles both `Id` and `Alias` variants
/// and degrades gracefully when fields are missing — we just need to flip the
/// empty case from `Some("")` to `None` so callers can drop the `(model)`
/// suffix from the role header entirely.
fn render_model_id(id: &PartialModelIdOrAliasConfig) -> Option<String> {
    let s = id.to_string();
    if s.is_empty() { None } else { Some(s) }
}
