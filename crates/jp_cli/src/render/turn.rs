//! Turn-level rendering for conversation replay.
//!
//! The [`TurnRenderer`] coordinates the [`ChatRenderer`], [`StructuredRenderer`],
//! and [`ToolRenderer`] to render a complete conversation event stream. It
//! handles turn boundaries, content-kind transitions, and tool config lookups.

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

use super::{ChatRenderer, StructuredRenderer, ToolRenderer, metadata::get_rendered_arguments};

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
/// Owns the three sub-renderers and dispatches each event to the right one,
/// handling turn separators, content-kind transitions, and tool config lookups.
pub struct TurnRenderer {
    // Stable params for rebuilding sub-renderers.
    printer: Arc<Printer>,
    root: Utf8PathBuf,
    is_tty: bool,
    source: ConfigSource,

    // Sub-renderers (rebuilt per-turn in PerTurn mode).
    chat: ChatRenderer,
    structured: StructuredRenderer,
    tool: ToolRenderer,
    tools_config: ToolsConfig,

    /// Maps tool call IDs to tool names, populated as `ToolCallRequest`
    /// events are encountered so that `ToolCallResponse` can look up the
    /// name without needing access to the full conversation stream.
    tool_names: HashMap<String, String>,
    is_first_turn: bool,
}

impl TurnRenderer {
    pub fn new(
        printer: Arc<Printer>,
        style: StyleConfig,
        tools_config: ToolsConfig,
        root: Utf8PathBuf,
        is_tty: bool,
        source: ConfigSource,
    ) -> Self {
        Self {
            chat: ChatRenderer::new(printer.clone(), style.clone()),
            structured: StructuredRenderer::new(printer.clone()),
            tool: ToolRenderer::new(printer.clone(), style, root.clone(), is_tty),
            printer,
            root,
            is_tty,
            source,
            tools_config,
            tool_names: HashMap::new(),
            is_first_turn: true,
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
                    if !self.is_first_turn {
                        self.chat.render_separator();
                    }
                    self.is_first_turn = false;
                }

                EventKind::ChatRequest(req) => {
                    self.chat.render_request(&req.content);
                }

                EventKind::ChatResponse(resp) => {
                    if resp.is_structured() {
                        self.chat.flush();
                        self.structured.render_chunk(resp);
                        self.structured.flush();
                    } else {
                        self.chat.render_response(resp);
                    }
                }

                EventKind::ToolCallRequest(req) => {
                    self.tool_names.insert(req.id.clone(), req.name.clone());

                    let default_style = &self.tools_config.defaults.style;
                    let tool_cfg = self.tools_config.get(&req.name);
                    let style = tool_cfg
                        .as_ref()
                        .map_or(default_style, ToolConfigWithDefaults::style);

                    if style.hidden {
                        // Tool call is hidden, but it's still a semantic
                        // boundary between message blocks. Flush the chat
                        // buffer so surrounding message chunks render as
                        // distinct paragraphs, without transitioning to
                        // ToolCall state (which would add an extra blank
                        // line on the next message, even though no tool UI
                        // was rendered).
                        self.chat.flush();
                    } else {
                        self.chat.flush();
                        self.chat.transition_to_tool_call();
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
        self.chat.flush();
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

        self.chat = ChatRenderer::new(self.printer.clone(), style.clone());
        self.structured = StructuredRenderer::new(self.printer.clone());
        self.tool = ToolRenderer::new(self.printer.clone(), style, self.root.clone(), self.is_tty);
        self.tools_config = config.conversation.tools;
    }
}
