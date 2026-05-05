//! Turn-level rendering for conversation replay.
//!
//! The [`TurnRenderer`] coordinates the [`TurnView`] (which owns the chat
//! and structured sub-renderers) and the [`ToolRenderer`] to render a
//! complete conversation event stream. It handles turn boundaries,
//! per-turn config rebuilds, and tool config lookups.

use std::{collections::HashMap, sync::Arc};

use camino::Utf8PathBuf;
use jp_config::{
    AppConfig, PartialAppConfig,
    conversation::tool::{ToolConfigWithDefaults, ToolsConfig, style::ParametersStyle},
    style::{StyleConfig, typewriter::DelayDuration},
};
use jp_conversation::{EventKind, stream::turn_iter::Turn};
use jp_printer::Printer;
use tracing::warn;

use super::{ToolRenderer, TurnView, metadata::get_rendered_arguments};

/// Controls where the renderer sources its configuration from.
#[derive(Debug, Clone)]
pub enum ConfigSource {
    /// Use the config as it was when each turn was created.
    ///
    /// The renderer rebuilds its sub-renderers at each turn boundary using
    /// the accumulated `PartialAppConfig` from the event stream.
    PerTurn,

    /// Use a fixed config for all turns (the current workspace config).
    Fixed,
}

/// Renders conversation events for replay (e.g. `jp conversation print`).
///
/// Owns a [`TurnView`] for chat/structured rendering and a [`ToolRenderer`]
/// for tool UI; dispatches each event to the right one and rebuilds the
/// view at turn boundaries when in [`ConfigSource::PerTurn`] mode.
pub struct TurnRenderer {
    // Stable params for rebuilding sub-renderers.
    printer: Arc<Printer>,
    root: Utf8PathBuf,
    is_tty: bool,
    source: ConfigSource,

    view: TurnView,
    tool: ToolRenderer,
    tools_config: ToolsConfig,

    /// Maps tool call IDs to tool names, populated as `ToolCallRequest`
    /// events are encountered so that `ToolCallResponse` can look up the
    /// name without needing access to the full conversation stream.
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
    ) -> Self {
        let view = TurnView::new(printer.clone(), style.clone(), assistant_name, model_id);
        let tool = ToolRenderer::new(printer.clone(), style, root.clone(), is_tty);
        Self {
            printer,
            root,
            is_tty,
            source,
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
                    let style = tool_cfg
                        .as_ref()
                        .map_or(default_style, ToolConfigWithDefaults::style);

                    if !style.hidden {
                        self.tool.render_result(
                            resp,
                            &style.inline_results,
                            &style.results_file_link,
                        );
                    }
                }

                EventKind::InquiryRequest(_) | EventKind::InquiryResponse(_) => {}
            }
        }
    }

    /// Flush all sub-renderers. Call after the last turn has been rendered.
    pub fn flush(&mut self) {
        self.view.flush();
    }

    /// Rebuild sub-renderers from a per-turn config partial.
    fn reconfigure(&mut self, partial: &PartialAppConfig) {
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

        let model_id = Some(config.assistant.model.id.resolved().to_string());
        self.view.reconfigure(
            self.printer.clone(),
            style.clone(),
            config.assistant.name,
            model_id,
        );
        self.tool = ToolRenderer::new(self.printer.clone(), style, self.root.clone(), self.is_tty);
        self.tools_config = config.conversation.tools;
    }
}
