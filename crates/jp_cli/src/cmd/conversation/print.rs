use jp_config::AppConfig;
use jp_conversation::{
    ConversationId, ConversationStream, EventKind,
    event::{ChatResponse, ToolCallResponse},
};
use jp_md::format::Formatter;
use jp_printer::Printer;

use crate::{
    cmd::{Output, query::tool::ToolRenderer},
    ctx::Ctx,
};

#[derive(Debug, clap::Args)]
pub(crate) struct Print {
    /// Conversation ID to print. Defaults to active conversation.
    id: Option<ConversationId>,
}

impl Print {
    pub(crate) async fn run(self, ctx: &mut Ctx) -> Output {
        let active_id = ctx.workspace.active_conversation_id();
        let id = self.id.unwrap_or(active_id);
        let events = ctx.workspace.try_get_events(&id)?.clone();
        let cfg = ctx.config();
        let pretty = ctx.printer.pretty_printing();

        let printer = ctx.printer.clone();
        let root = ctx
            .workspace
            .storage_path()
            .unwrap_or(ctx.workspace.root())
            .to_path_buf();

        let tool_renderer = ToolRenderer::new(
            printer.clone(),
            cfg.style.clone(),
            root.clone(),
            ctx.term.is_tty,
        );

        let mut is_first_turn = true;

        for event_with_cfg in events.iter() {
            match &event_with_cfg.event.kind {
                EventKind::TurnStart(_) => {
                    if !is_first_turn {
                        printer.println("\n---\n");
                    }
                    is_first_turn = false;
                }

                EventKind::ChatRequest(req) => {
                    render_user_message(&printer, &cfg, pretty, &req.content)?;
                }

                EventKind::ChatResponse(resp) => {
                    render_chat_response(&printer, &cfg, pretty, resp)?;
                }

                EventKind::ToolCallRequest(req) => {
                    let tool_cfg = cfg.conversation.tools.get(&req.name);
                    if !tool_cfg.as_ref().is_some_and(|c| c.style().hidden) {
                        let params_style = tool_cfg
                            .as_ref()
                            .map(|c| c.style().parameters.clone())
                            .unwrap_or_default();
                        tool_renderer.render_call_header(&req.name);
                        if let Err(e) = tool_renderer
                            .render_arguments(&req.name, &req.arguments, &params_style)
                            .await
                        {
                            tracing::warn!(error = %e, tool = %req.name, "Failed to format tool arguments");
                            printer.println("");
                        }
                    }
                }

                EventKind::ToolCallResponse(resp) => {
                    let name = find_tool_name_for_response(&events, resp);
                    let tool_cfg = name.as_deref().and_then(|n| cfg.conversation.tools.get(n));
                    if !tool_cfg.as_ref().is_some_and(|c| c.style().hidden) {
                        let inline = tool_cfg
                            .as_ref()
                            .map(|c| c.style().inline_results.clone())
                            .unwrap_or_default();
                        let link = tool_cfg
                            .as_ref()
                            .map(|c| c.style().results_file_link.clone())
                            .unwrap_or_default();
                        tool_renderer.render_result(resp, &inline, &link);
                    }
                }

                EventKind::InquiryRequest(_) | EventKind::InquiryResponse(_) => {}
            }
        }

        printer.println("");
        printer.flush();

        Ok(())
    }
}

/// Format and print a user message (`ChatRequest`).
fn render_user_message(
    printer: &Printer,
    cfg: &AppConfig,
    pretty: bool,
    content: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let formatter = build_formatter(cfg, pretty);
    let formatted = formatter.format_terminal(&format!("{content}\n\n---\n\n"))?;
    printer.println(formatted);
    Ok(())
}

/// Render a `ChatResponse`, respecting reasoning display config.
fn render_chat_response(
    printer: &Printer,
    cfg: &AppConfig,
    pretty: bool,
    response: &ChatResponse,
) -> Result<(), Box<dyn std::error::Error>> {
    match response {
        ChatResponse::Message { message } => {
            let formatter = build_formatter(cfg, pretty);
            let formatted = formatter.format_terminal(message)?;
            printer.print(formatted);
        }

        ChatResponse::Reasoning { reasoning } => {
            render_reasoning(printer, cfg, pretty, reasoning)?;
        }

        ChatResponse::Structured { data } => {
            if let Ok(pretty) = serde_json::to_string_pretty(data) {
                printer.println(format!("```json\n{pretty}\n```\n"));
            }
        }
    }

    Ok(())
}

/// Render reasoning content according to the display config.
fn render_reasoning(
    printer: &Printer,
    cfg: &AppConfig,
    pretty: bool,
    content: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    use jp_config::style::reasoning::ReasoningDisplayConfig;

    match cfg.style.reasoning.display {
        ReasoningDisplayConfig::Hidden => {}

        ReasoningDisplayConfig::Full => {
            let formatter = build_formatter(cfg, pretty);
            let formatted = formatter.format_terminal(content)?;
            printer.print(formatted);
        }

        ReasoningDisplayConfig::Truncate(ref trunc) => {
            let truncated: String = content.chars().take(trunc.characters).collect();
            let suffix = if content.chars().count() > trunc.characters {
                "..."
            } else {
                ""
            };
            let formatter = build_formatter(cfg, pretty);
            let formatted = formatter.format_terminal(&format!("{truncated}{suffix}\n\n"))?;
            printer.print(formatted);
        }

        // Progress/Static/Summary modes don't apply to replay — show a label.
        ReasoningDisplayConfig::Progress
        | ReasoningDisplayConfig::Static
        | ReasoningDisplayConfig::Summary => {
            printer.println("reasoning...\n");
        }
    }

    Ok(())
}

/// Look up the tool name for a `ToolCallResponse` by finding its matching request.
fn find_tool_name_for_response(
    events: &ConversationStream,
    resp: &ToolCallResponse,
) -> Option<String> {
    events.iter().find_map(|e| {
        e.event
            .as_tool_call_request()
            .filter(|req| req.id == resp.id)
            .map(|req| req.name.clone())
    })
}

fn build_formatter(cfg: &AppConfig, pretty: bool) -> Formatter {
    Formatter::with_width(cfg.style.markdown.wrap_width)
        .table_max_column_width(cfg.style.markdown.table_max_column_width)
        .theme(if pretty {
            cfg.style.markdown.theme.as_deref()
        } else {
            None
        })
        .pretty_hr(pretty && cfg.style.markdown.hr_style.is_line())
}

#[cfg(test)]
#[path = "print_tests.rs"]
mod tests;
