use std::{env, fs, time};

use crossterm::style::Stylize as _;
use jp_config::conversation::tool::{
    style::{InlineResults, LinkStyle, Truncate},
    ToolConfigWithDefaults,
};
use jp_conversation::message::{ToolCallRequest, ToolCallResult};
use jp_llm::CompletionChunk;
use jp_term::osc::hyperlink;
use serde_json::Value;

use super::ResponseHandler;
use crate::{Ctx, Error};

pub(super) struct StreamEventHandler {
    pub reasoning_tokens: String,
    pub content_tokens: String,
    pub tool_calls: Vec<ToolCallRequest>,
    pub tool_call_results: Vec<ToolCallResult>,
}

impl StreamEventHandler {
    pub(super) fn handle_chat_chunk(
        &mut self,
        ctx: &mut Ctx,
        chunk: CompletionChunk,
    ) -> Option<String> {
        match chunk {
            CompletionChunk::Reasoning(data) if !data.is_empty() => {
                self.reasoning_tokens.push_str(&data);

                if !ctx.config().style.reasoning.show {
                    return None;
                }

                Some(data)
            }
            CompletionChunk::Content(mut data) if !data.is_empty() => {
                let reasoning_ended = !self.reasoning_tokens.is_empty()
                    && ctx.config().style.reasoning.show
                    && self.content_tokens.is_empty();

                self.content_tokens.push_str(&data);

                // If the response includes reasoning, we add two newlines
                // after the reasoning, but before the content.
                if reasoning_ended {
                    data = format!("\n\n{data}");
                }

                Some(data)
            }
            _ => None,
        }
    }

    pub async fn handle_tool_call(
        &mut self,
        ctx: &mut Ctx,
        call: ToolCallRequest,
        handler: &mut ResponseHandler,
    ) -> Result<Option<String>, Error> {
        let Some(tool_config) = ctx.config().conversation.tools.get(&call.name) else {
            return Err(Error::NotFound("tool", call.name.clone()));
        };

        let editor = ctx.config().editor.path().ok_or(Error::MissingEditor)?;

        self.tool_calls.push(call.clone());
        let tool = jp_llm::tool::ToolDefinition::new(
            &call.name,
            tool_config.source(),
            tool_config.description().map(str::to_owned),
            tool_config.parameters().clone(),
            &ctx.mcp_client,
        )
        .await?;

        if handler.render_tool_calls {
            let mut title = format!("Calling tool **{}**", tool.name);

            if let Value::Object(map) = &call.arguments
                && !map.is_empty()
            {
                let arguments = serde_json::to_string_pretty(&call.arguments)?;
                title.push_str(&format!(" with arguments:\n\n```json\n{arguments}\n```\n"));
            }

            let data = format!("\n{title}\n");
            handler.handle(&data, ctx, false)?;
        }

        let result = tool
            .call(
                call.id,
                call.arguments,
                &ctx.mcp_client,
                tool_config.clone(),
                ctx.workspace.root.clone(),
                editor,
            )
            .await?;

        self.tool_call_results.push(result.clone());

        build_tool_call_result(ctx, &result, &tool_config, handler)
    }
}

fn build_tool_call_result(
    ctx: &Ctx,
    result: &ToolCallResult,
    tool_config: &ToolConfigWithDefaults,
    handler: &mut ResponseHandler,
) -> Result<Option<String>, Error> {
    let content = if let Ok(json) = serde_json::from_str::<Value>(result.content.trim()) {
        format!("```json\n{}\n```", serde_json::to_string_pretty(&json)?)
    } else {
        result.content.trim().to_owned()
    };

    let mut lines = content.lines().collect::<Vec<_>>();
    let mut ext = lines.first().and_then(|v| v.strip_prefix("```")).map(|v| {
        v.chars()
            .take_while(char::is_ascii_alphabetic)
            .collect::<String>()
    });

    if ext.is_some() {
        lines.remove(0);
    }

    if lines.last().is_some_and(|v| v.ends_with("```")) {
        lines.pop();
    }

    // See if we can detect the language by parsing the content.
    //
    // We only do this for "container" formats (e.g. XML starting with `<` or
    // JSON starting with `{`) to avoid applying this too aggressively (e.g. a
    // quoted string should not be treated as JSON unless explicitly defined as
    // such).
    if ext.is_none() {
        if content.trim().starts_with('<')
            && quick_xml::de::from_str::<serde_json::Value>(&content).is_ok()
        {
            ext = Some("xml".to_owned());
        } else if content.trim().starts_with('{')
            && serde_json::from_str::<serde_json::Value>(&content).is_ok()
        {
            ext = Some("json".to_owned());
        }
    }

    let content = lines.join("\n");

    let millis = time::SystemTime::now()
        .duration_since(time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_millis();

    let file_name = match ext.as_ref() {
        Some(ext) => format!("tool_call_{millis}.{ext}"),
        None => format!("tool_call_{millis}"),
    };

    let path = env::temp_dir().join(file_name);
    fs::write(&path, &content)?;

    let max_lines = match tool_config.style().inline_results {
        InlineResults::Truncate(Truncate { lines }) => lines,
        _ => content.lines().count(),
    };

    let mut intro = "\nTool call result".to_owned();
    match tool_config.style().inline_results {
        InlineResults::Truncate(Truncate { lines }) if lines < content.lines().count() => {
            intro.push_str(&format!(" _(truncated to {lines} lines)_"));
        }
        _ => {}
    }
    intro.push_str(":\n");

    handler.handle(&intro, ctx, false)?;

    let mut data = "\n".to_owned();

    if let Some(ext) = ext.as_ref() {
        data.push_str("```");
        data.push_str(ext);
        data.push('\n');
    }

    for line in content.lines().take(max_lines) {
        data.push_str(line);
        data.push('\n');
    }

    if ext.is_some() {
        data.push_str("```");
    }

    if matches!(tool_config.style().inline_results, InlineResults::Off) {
        data.clear();
    }

    if handler.render_tool_calls {
        if !data.ends_with('\n') {
            data.push('\n');
        }

        handler.handle(&data, ctx, false)?;
    }

    let link = match tool_config.style().results_file_link {
        LinkStyle::Off => None,
        LinkStyle::Full => Some(format!("see: {}\n\n", path.display())),
        LinkStyle::Osc8 => Some(format!(
            "[{}] [{}]\n\n",
            hyperlink(
                format!("file://{}", path.display()),
                "open in editor".red().to_string()
            ),
            hyperlink(
                format!("copy://{}", path.display()),
                "copy to clipboard".red().to_string()
            )
        )),
    };

    if handler.render_tool_calls
        && let Some(link) = link
    {
        handler.handle(&link, ctx, true)?;
    }

    Ok(None)
}

pub(super) async fn handle_tool_calls(
    ctx: &Ctx,
    tool_calls: Vec<ToolCallRequest>,
) -> Result<Vec<ToolCallResult>, Error> {
    let mut results = vec![];

    for call in tool_calls {
        let Some(tool_config) = ctx.config().conversation.tools.get(&call.name) else {
            return Err(Error::NotFound("tool", call.name.clone()));
        };

        let tool = jp_llm::tool::ToolDefinition::new(
            &call.name,
            tool_config.source(),
            tool_config.description().map(str::to_owned),
            tool_config.parameters().clone(),
            &ctx.mcp_client,
        )
        .await?;
        let editor = ctx.config().editor.path().ok_or(Error::MissingEditor)?;

        results.push(
            tool.call(
                call.id,
                call.arguments,
                &ctx.mcp_client,
                tool_config,
                ctx.workspace.root.clone(),
                editor,
            )
            .await?,
        );
    }

    Ok(results)
}
