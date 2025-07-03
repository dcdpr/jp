use std::{
    fs,
    path::{Path, PathBuf},
};

use crossterm::style::Stylize as _;
use hex::ToHex as _;
use inquire::Confirm;
use jp_config::{
    mcp::server::{checksum::Algorithm, tool},
    style::LinkStyle,
};
use jp_conversation::message::{ToolCallRequest, ToolCallResult};
use jp_llm::CompletionChunk;
use jp_mcp::{tool::McpToolId, CallToolResult, Content, ResourceContents};
use jp_term::osc::hyperlink;
use open_editor::{edit_mut_in_editor_with_opts, EditOptions};
use serde_json::Value;
use sha1::{Digest as _, Sha1};
use sha2::Sha256;
use tracing::{info, trace};

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

                if !ctx.config.style.reasoning.show {
                    return None;
                }

                Some(data)
            }
            CompletionChunk::Content(mut data) if !data.is_empty() => {
                let reasoning_ended = !self.reasoning_tokens.is_empty()
                    && ctx.config.style.reasoning.show
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
        self.tool_calls.push(call.clone());

        let data = indoc::formatdoc!(
            "\n\n
                    ---
                    calling tool: **{}**

                    arguments:
                    ```json
                    {:#}
                    ```

                ",
            call.name,
            call.arguments
        );

        handler.handle(&data, ctx)?;
        let result = handle_tool_call(ctx, call.clone()).await?;
        self.tool_call_results.push(result.clone());

        // FIXME: Need to add `output: ...\n---` and render it
        if result.content.len() > 10_000 {
            let ext = result
                .content
                .lines()
                .next()
                .map_or("txt", |v| v.trim_start_matches("```"))
                .chars()
                .take(10)
                .collect::<String>();

            let millis = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_millis();

            let path = std::env::temp_dir().join(format!("tool_call_{millis}.{ext}"));
            fs::write(&path, &result.content)?;

            let data = match ctx.config.style.code.file_link {
                LinkStyle::Off => return Ok(None),
                LinkStyle::Full => {
                    format!("large result omitted, see: file://{}\n", path.display())
                }
                LinkStyle::Osc8 => {
                    format!(
                        "[{}]\n",
                        hyperlink(
                            format!("file://{}", path.display()),
                            "large result omitted, open in editor".red().to_string()
                        )
                    )
                }
            };

            handler.handle(&data, ctx)?;

            Ok(None)
        } else {
            let content = if result.content.starts_with("```") || result.content.trim().is_empty() {
                result.content
            } else {
                format!("```\n{}\n```", result.content)
            };

            Ok(Some(indoc::formatdoc! {"
                            result:

                            {content}
                            ---
                            "
            }))
        }
    }
}

#[expect(clippy::too_many_lines)]
async fn handle_tool_call(ctx: &Ctx, mut call: ToolCallRequest) -> Result<ToolCallResult, Error> {
    info!(tool = %call.name, arguments = %call.arguments, "Calling tool.");
    let tool_id = McpToolId::new(&call.name);
    let server_id = ctx.mcp_client.get_tool_server_id(&tool_id).await?;
    let server_cfg = ctx.config.mcp.get_server_with_defaults(server_id.as_str());
    let mut tool_cfg = server_cfg.get_tool_with_defaults(&call.name);

    if let Some(checksum) = &server_cfg.binary_checksum {
        let path = get_tool_binary_path(ctx, &tool_id).await?;
        verify_file_checksum(&tool_id, &path, &checksum.value, checksum.algorithm)?;
    }

    if matches!(tool_cfg.run, tool::RunMode::Edit) {
        let editor = ctx.config.editor.command().ok_or(Error::MissingEditor)?;
        let args = serde_json::to_string_pretty(&call.arguments)?;

        call.arguments = loop {
            let args = open_editor::edit_in_editor_with_opts(&args, EditOptions {
                editor: Some(open_editor::Editor::from(&editor)),
                ..Default::default()
            })
            .map_err(|e| Error::Editor(e.to_string()))?;

            // If the user removed all data from the argument, we consider the
            // edit a no-op, and ask the user if they want to run the tool.
            if args.is_empty() {
                tool_cfg.run = tool::RunMode::Ask;
                break serde_json::json!({});
            }

            // If we can't parse the arguments as valid JSON, we consider the
            // input invalid, and ask the user if they want to re-open the
            // editor.
            match serde_json::from_str::<Value>(&args) {
                Ok(value) => break value,
                Err(error) => {
                    let retry = Confirm::new("Re-open editor?")
                        .with_default(true)
                        .with_help_message(&format!("JSON parsing error: {error}"))
                        .prompt()
                        .unwrap_or(false);

                    if !retry {
                        return Err(error.into());
                    }
                }
            }
        };
    }

    let should_run = match tool_cfg.run {
        tool::RunMode::Ask => Confirm::new(&format!(
            "Run {} tool by {} server?",
            call.name.clone().bold().yellow(),
            server_id.as_str().bold().blue(),
        ))
        .with_default(true)
        .prompt()
        .unwrap_or(false),
        _ => true,
    };

    let mut result = if should_run {
        ctx.mcp_client.call_tool(&call.name, call.arguments).await?
    } else {
        CallToolResult::success(vec![Content::text("Tool call rejected by user.")])
    };

    trace!(result = ?result, "Tool call completed.");

    if matches!(tool_cfg.result, tool::ResultMode::Edit) {
        let editor = ctx.config.editor.command().ok_or(Error::MissingEditor)?;
        let mut text = result
            .content
            .iter()
            .filter_map(|c| c.as_text().map(|t| t.text.clone()))
            .collect::<Vec<_>>()
            .join("\n\n");

        edit_mut_in_editor_with_opts(&mut text, EditOptions {
            editor: Some(open_editor::Editor::from(editor)),
            ..Default::default()
        })
        .map_err(|e| Error::Editor(e.to_string()))?;

        result.content = text.split("\n\n").map(Content::text).collect();
    }

    let should_deliver = match tool_cfg.result {
        tool::ResultMode::Ask => Confirm::new(&format!(
            "Deliver the results of the {} tool call?",
            call.name.clone().bold().yellow(),
        ))
        .with_default(true)
        .prompt()
        .unwrap_or(false),
        _ => true,
    };

    let result = if should_deliver {
        result
    } else {
        CallToolResult::success(vec![Content::text(
            "Tool call results omitted.".to_string(),
        )])
    };

    Ok(ToolCallResult {
        id: call.id,
        error: result.is_error.unwrap_or(false),
        content: result
            .content
            .into_iter()
            .filter_map(|c| match c.raw {
                jp_mcp::RawContent::Text(text_content) => Some(text_content.text),
                jp_mcp::RawContent::Resource(embedded_resource) => {
                    match embedded_resource.resource {
                        ResourceContents::TextResourceContents { text, .. } => Some(text),
                        ResourceContents::BlobResourceContents { .. } => None,
                    }
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n\n"),
    })
}

pub(super) async fn handle_tool_calls(
    ctx: &Ctx,
    tool_calls: Vec<ToolCallRequest>,
) -> Result<Vec<ToolCallResult>, Error> {
    let mut results = vec![];
    for call in tool_calls {
        results.push(handle_tool_call(ctx, call).await?);
    }

    Ok(results)
}

pub fn verify_file_checksum(
    tool_id: &McpToolId,
    path: &Path,
    hash: &str,
    algo: Algorithm,
) -> Result<(), Error> {
    let contents = fs::read(path)?;
    let digest = match algo {
        Algorithm::Sha256 => format!("{:x}", Sha256::digest(&contents)),
        Algorithm::Sha1 => format!("{:x}", Sha1::digest(&contents)),
    };

    if digest.eq_ignore_ascii_case(hash) {
        return Ok(());
    }

    Err(Error::Mcp(jp_mcp::Error::ChecksumMismatch {
        tool: tool_id.to_string(),
        path: path.to_path_buf(),
        expected: hash.to_string(),
        got: digest.encode_hex(),
    }))
}

/// Get the path to the binary for the given server, if the server is using
/// the `stdio` transport.
async fn get_tool_binary_path(ctx: &Ctx, id: &McpToolId) -> Result<PathBuf, Error> {
    let server_id = ctx.mcp_client.get_tool_server_id(id).await?;
    let path = if server_id.as_str() == "embedded" {
        ctx.mcp_client.get_embedded_tool_path(id).await?
    } else {
        let server = ctx
            .workspace
            .get_mcp_server(&server_id)
            .ok_or(Error::Mcp(jp_mcp::Error::UnknownServer(server_id.clone())))?;

        let jp_mcp::transport::Transport::Stdio(transport) = &server.transport;

        transport.command.clone()
    };

    if path.exists() {
        return Ok(path);
    }

    which::which(path).map_err(Into::into)
}
