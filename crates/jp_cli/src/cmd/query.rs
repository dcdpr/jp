use std::{
    collections::HashSet, convert::Infallible, fs, path::PathBuf, str::FromStr as _, time::Duration,
};

use clap::builder::TypedValueParser as _;
use crossterm::style::{Color, Stylize as _};
use futures::StreamExt as _;
use jp_config::{llm::ToolChoice, parse_vec, style::code::LinkStyle};
use jp_conversation::{
    message::{ToolCallRequest, ToolCallResult},
    persona::Instructions,
    thread::{Thread, ThreadBuilder},
    AssistantMessage, ContextId, Conversation, ConversationId, MessagePair, Model, ModelId,
    PersonaId, UserMessage,
};
use jp_llm::provider::{self, CompletionChunk, StreamEvent};
use jp_mcp::{config::McpServerId, ResourceContents, Tool};
use jp_query::query::{ChatQuery, StructuredQuery};
use jp_task::task::TitleGeneratorTask;
use jp_term::{code, osc::hyperlink, stdout};
use minijinja::{Environment, UndefinedBehavior};
use termimad::FmtText;
use tracing::{debug, info, trace};
use url::Url;

use super::{attachment::register_attachment, Output};
use crate::{
    cmd::Success,
    editor,
    error::{Error, Result},
    parser, Ctx, PATH_STRING_PREFIX,
};

// Define the delay duration
const TYPEWRITER_DELAY: Duration = Duration::from_millis(3);

#[derive(Debug, clap::Args)]
pub struct Args {
    /// The query to send. If not provided, uses `$JP_EDITOR`, `$VISUAL` or
    /// `$EDITOR` to open edit the query in an editor.
    #[arg(value_parser = string_or_path)]
    pub query: Option<String>,

    /// Use the query string as a Jinja2 template.
    ///
    /// You can provide values for template variables using the
    /// `template.values` config key.
    #[arg(short, long)]
    pub template: bool,

    #[arg(long, value_parser = string_or_path.try_map(json_schema))]
    pub schema: Option<schemars::Schema>,

    /// Replay the last message in the conversation.
    ///
    /// If a query is provided, it will be appended to the end of the previous
    /// message. If no query is provided, $EDITOR will open with the last
    /// message in the conversation.
    #[arg(short = 'r', long = "replay", conflicts_with = "new_conversation")]
    pub replay: bool,

    /// Start a new conversation without any message history.
    ///
    /// If a context named `default` exists, it will be attached to the
    /// conversation.
    #[arg(short = 'n', long = "new")]
    pub new_conversation: bool,

    /// Store the conversation locally, outside of the workspace.
    #[arg(short = 'l', long = "local", requires = "new_conversation")]
    pub local: bool,

    /// Add attachment to the context.
    #[arg(short = 'a', long = "attachment", value_parser = |s: &str| parser::attachment_url(s))]
    pub attachments: Vec<Url>,

    /// Use specific persona.
    #[arg(short = 'p', long = "persona", value_parser = PersonaId::from_str)]
    pub persona: Option<PersonaId>,

    /// Use specific context.
    #[arg(short = 'x', long = "context", value_parser = |s: &str| ContextId::try_from(s))]
    pub context: Option<ContextId>,

    /// Use specific MCP servers exclusively.
    #[arg(short = 'm', long = "mcp", value_parser = |s: &str| Ok::<_, Infallible>(parse_vec(s, McpServerId::new)))]
    pub mcp: Vec<McpServerId>,
}

impl Args {
    #[expect(clippy::too_many_lines)]
    pub async fn run(self, ctx: &mut Ctx) -> Output {
        debug!("Running `query` command.");
        trace!(args = ?self, "Received arguments.");

        self.update_config(&mut ctx.config);

        let old_conversation_id = ctx.workspace.active_conversation_id();
        let conversation_id = if self.new_conversation {
            let mut conversation = Conversation::default();
            if self.local {
                conversation.local = true;
            }

            let id = ctx.workspace.create_conversation(conversation);
            debug!(
                id = %id,
                local = %self.local,
                "Creating new active conversation due to --new flag."
            );

            ctx.workspace.set_active_conversation_id(id)?;
            id
        } else {
            ctx.workspace.active_conversation_id()
        };

        // Update the conversation context based on the contextual information
        // passed in through the CLI, configuration, and environment variables.
        self.update_context(ctx).await?;

        // Ensure we start the MCP servers attached to the conversation.
        ctx.configure_active_mcp_servers().await?;

        let message = self.build_message(ctx, conversation_id).await?;

        if let UserMessage::Query(query) = &message {
            if query.is_empty() {
                info!("Query is empty, exiting.");

                if old_conversation_id != conversation_id {
                    ctx.workspace
                        .set_active_conversation_id(old_conversation_id)?;
                    ctx.workspace.remove_conversation(&conversation_id)?;
                }

                let path = ctx.workspace.storage_path().unwrap_or(&ctx.workspace.root);
                editor::cleanup_query_file(path)?;

                return Ok("Query is empty, ignoring.".into());
            }

            // Generate title for new conversations.
            if self.new_conversation && ctx.config.conversation.title.generate.auto {
                debug!("Generating title for new conversation");
                ctx.task_handler.spawn(TitleGeneratorTask::new(
                    conversation_id,
                    &ctx.config,
                    &ctx.workspace,
                    Some(query.clone()),
                ));
            }
        }

        // Conversation
        let conversation = ctx.workspace.get_active_conversation();

        // Persona
        let persona_id = &conversation.context.persona_id;
        let Some(persona) = ctx.workspace.get_persona(persona_id) else {
            return Err(Error::NotFound("Persona", persona_id.to_string()).into());
        };

        // Model
        let mut model = ctx
            .workspace
            .resolve_model_reference(&persona.model)?
            .clone();

        // For explicit model requests, try to fetch the model configuration
        // from the workspace, otherwise use a default model with the requested
        // provider and model name.
        if let Some(explicit_model) = ctx.config.llm.model.clone() {
            let id = ModelId::try_from((explicit_model.provider, &explicit_model.slug))?;
            model = ctx.workspace.get_model(&id).cloned().unwrap_or(Model {
                provider: explicit_model.provider,
                slug: explicit_model.slug,
                ..Default::default()
            });
        }

        trace!(provider = %model.provider, slug = %model.slug, "Loaded LLM model.");

        // Attachments
        let mut attachments = vec![];
        for handler in conversation.context.attachment_handlers.values() {
            attachments.extend(handler.get(&ctx.workspace.root).await?);
        }

        // Messages
        let messages = ctx.workspace.get_messages(&conversation_id);
        let tools = ctx.mcp_client.list_tools().await?;
        let mut thread_builder = ThreadBuilder::default()
            .with_system_prompt(persona.system_prompt.clone())
            .with_instructions(persona.instructions.clone())
            .with_attachments(attachments)
            .with_history(messages.to_vec())
            .with_message(message.clone());

        if !tools.is_empty() {
            let instruction = Instructions::default()
                .with_title("Tool Usage")
                .with_description("How to leverage the tools available to you.".to_string())
                .with_item("Use all the tools available to you to give the best possible answer.")
                .with_item("Verify the tool name, description and parameters are correct.")
                .with_item(
                    "Even if you've reasoned yourself towards a solution, use any available tool \
                     to verify your answer.",
                );

            thread_builder = thread_builder.with_instruction(instruction);
        }

        let mut thread = thread_builder.build()?;
        let context = conversation.context.clone();
        let reply = if let Some(schema) = &self.schema {
            handle_structured_output(ctx, thread.clone(), &model, schema.clone()).await?
        } else {
            handle_stream(ctx, thread.clone(), &model, tools.clone()).await?
        };

        trace!(
            conversation = %conversation_id,
            content_size = reply.content.as_deref().unwrap_or_default().len(),
            reasoning_size = reply.reasoning.as_deref().unwrap_or_default().len(),
            "Storing response message in conversation."
        );

        let tool_calls = reply.tool_calls.clone();
        let message = MessagePair::new(message.clone(), reply.clone()).with_context(context);

        // Create message in the conversation.
        thread.history.push(message.clone());
        ctx.workspace.add_message(conversation_id, message);

        // If the assistant asked for a tool call, we handle it automatically,
        // essentially going into a "loop" until no more tool calls are requested.
        //
        // TODO:
        //
        // This should be handled differently, asking for permission to run a tool
        // (unless whitelisted per conversation/globally), it should log the fact
        // that a tool call is triggered, and it should guard against infinite
        // loops.
        if !tool_calls.is_empty() {
            let results = handle_tool_calls(ctx, tool_calls).await?;
            thread.message = UserMessage::ToolCallResults(results);
            Box::pin(handle_stream(ctx, thread, &model, tools)).await?;
        }

        // Clean up the query file.
        let path = ctx.workspace.storage_path().unwrap_or(&ctx.workspace.root);
        editor::cleanup_query_file(path)?;

        if self.schema.is_some() {
            if let Some(content) = reply.content {
                return Ok(Success::Json(serde_json::from_str(&content)?));
            }
        }

        Ok(Success::Ok)
    }

    async fn build_message(
        &self,
        ctx: &mut Ctx,
        conversation_id: ConversationId,
    ) -> Result<UserMessage> {
        // If replaying, remove the last message from the conversation, and use
        // its query message to build the new query.
        let replaying_user_message = self
            .replay
            .then(|| ctx.workspace.pop_message(&conversation_id))
            .flatten()
            .map(|m| m.message);

        let mut message = match replaying_user_message {
            Some(msg @ UserMessage::Query(_)) => msg,
            Some(UserMessage::ToolCallResults(_)) => {
                let Some(response) = ctx.workspace.get_messages(&conversation_id).last() else {
                    return Err(Error::Replay("No assistant response found".into()));
                };

                let results = handle_tool_calls(ctx, response.reply.tool_calls.clone()).await?;
                UserMessage::ToolCallResults(results)
            }
            None => UserMessage::Query(String::new()),
        };

        if let Some(text) = &self.query {
            match &mut message {
                UserMessage::Query(query) if query.is_empty() => text.clone_into(query),
                UserMessage::Query(query) => *query = format!("{text}\n\n{query}"),
                UserMessage::ToolCallResults(_) => {}
            }
        } else if let UserMessage::Query(query) = &mut message {
            let path = ctx.workspace.storage_path().unwrap_or(&ctx.workspace.root);
            let messages = ctx.workspace.get_messages(&conversation_id);
            let initial_message = if query.is_empty() {
                None
            } else {
                Some(query.to_owned())
            };

            // If replaying, pass the last query as the text to be edited,
            // otherwise open an empty editor.
            *query = editor::edit_query(path, initial_message, messages)?;
        }

        if let UserMessage::Query(query) = &mut message {
            if self.template {
                let mut env = Environment::empty();
                env.set_undefined_behavior(UndefinedBehavior::SemiStrict);
                env.add_template("query", query)?;

                let tmpl = env.get_template("query")?;
                // TODO: supported nested variables
                for var in tmpl.undeclared_variables(false) {
                    if ctx.config.template.values.contains_key(&var) {
                        continue;
                    }

                    return Err(Error::TemplateUndefinedVariable(var));
                }

                *query = tmpl.render(&ctx.config.template.values)?;
            }
        }

        Ok(message)
    }

    async fn update_context(&self, ctx: &mut Ctx) -> Result<()> {
        // Update context if specified
        if let Some(id) = ctx.config.conversation.context.clone() {
            debug!(
                %id,
                "Using named context in conversation due to conversation.context config."
            );

            // Get context.
            let context = ctx
                .workspace
                .get_named_context(&id)
                .ok_or(Error::NotFound("Context", id.to_string()))?
                .clone();

            // Update conversation context.
            ctx.workspace.get_active_conversation_mut().context = context;
        }

        // Update persona if specified
        if let Some(id) = ctx.config.conversation.persona.clone() {
            debug!(
                %id,
                "Changing persona in conversation context due to conversation.persona config."
            );

            // Ensure persona exists.
            ctx.workspace
                .get_persona(&id)
                .ok_or(Error::NotFound("Persona", id.to_string()))?;

            // Update context with new persona.
            ctx.workspace
                .get_active_conversation_mut()
                .context
                .persona_id = id;
        }

        // Add any new attachments specified in arguments
        for attachment in &self.attachments {
            let context = &mut ctx.workspace.get_active_conversation_mut().context;
            register_attachment(attachment, context).await?;
        }

        // Set exclusive MCP servers
        let mut servers = HashSet::new();
        for id in &self.mcp {
            // Ensure MCP server exists.
            ctx.workspace
                .get_mcp_server(id)
                .ok_or(Error::NotFound("MCP server", id.to_string()))?;

            servers.insert(id.clone());
        }

        if !servers.is_empty() {
            debug!(
                servers = servers
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", "),
                "Overriding MCP server in conversation context due to --mcp flag."
            );

            ctx.workspace
                .get_active_conversation_mut()
                .context
                .mcp_server_ids = servers;
        }

        Ok(())
    }

    fn update_config(&self, config: &mut jp_config::Config) {
        if let Some(context) = self.context.as_ref() {
            config.conversation.context = Some(context.clone());
        }

        if let Some(persona) = self.persona.as_ref() {
            config.conversation.persona = Some(persona.clone());
        }
    }
}

async fn handle_structured_output(
    ctx: &mut Ctx,
    thread: Thread,
    model: &Model,
    schema: schemars::Schema,
) -> Result<AssistantMessage> {
    let provider = provider::get_provider(model.provider, &ctx.config.llm.provider)?;
    let query =
        StructuredQuery::new(schema, thread).map_err(|err| Error::Schema(err.to_string()))?;

    let value = provider.structured_completion(model, query).await?;
    let content = if ctx.term.is_tty {
        serde_json::to_string_pretty(&value)?
    } else {
        serde_json::to_string(&value)?
    };

    Ok(AssistantMessage::from(content))
}

#[expect(clippy::needless_pass_by_value)]
fn json_schema(s: String) -> Result<schemars::Schema> {
    serde_json::from_str::<serde_json::Value>(&s)?
        .try_into()
        .map_err(Into::into)
}

fn string_or_path(s: &str) -> Result<String> {
    if let Some(s) = s.strip_prefix(PATH_STRING_PREFIX) {
        return fs::read_to_string(PathBuf::from(s.trim())).map_err(Into::into);
    }

    Ok(s.to_owned())
}

async fn handle_stream(
    ctx: &mut Ctx,
    thread: Thread,
    model: &Model,
    tools: Vec<Tool>,
) -> Result<AssistantMessage> {
    let provider = provider::get_provider(model.provider, &ctx.config.llm.provider)?;
    let query = ChatQuery {
        thread,
        tools: tools.clone(),
        tool_choice: ToolChoice::Auto,
        ..Default::default()
    };
    let mut stream = provider.chat_completion_stream(model, query)?;

    let mut content_tokens = String::new();
    let mut reasoning_tokens = String::new();
    let mut handler = ResponseHandler::default();
    let mut tool_calls = Vec::new();

    while let Some(event) = stream.next().await {
        let data = match event? {
            StreamEvent::ChatChunk(chunk) => match chunk {
                CompletionChunk::Reasoning(data) if !data.is_empty() => {
                    reasoning_tokens.push_str(&data);

                    data
                }
                CompletionChunk::Content(data) if !data.is_empty() => {
                    content_tokens.push_str(&data);

                    // If the response includes reasoning, we add two newlines
                    // after the reasoning, but before the content.
                    if !reasoning_tokens.is_empty() && content_tokens.is_empty() {
                        print!("\n\n");
                    }

                    data
                }
                _ => continue,
            },
            // Tool calls are handled after the stream is finished.
            //
            // We do add a history of the call to the content tokens for the
            // LLMs understanding, but we do not print it to the terminal.
            StreamEvent::ToolCall(call) => {
                tool_calls.push(call);
                continue;
            }
        };

        handler.handle_stream(&data, ctx)?;
    }

    // Ensure we handle the last line of the stream.
    if !handler.buffer.is_empty() {
        handler.handle_stream("\n", ctx)?;
    }

    let content_tokens = content_tokens.trim().to_string();
    let content = if !content_tokens.is_empty() {
        Some(content_tokens)
    } else if content_tokens.is_empty() && tool_calls.is_empty() {
        Some("<no reply>".to_string())
    } else {
        None
    };

    let reasoning_tokens = reasoning_tokens.trim().to_string();
    let reasoning = if reasoning_tokens.is_empty() {
        None
    } else {
        Some(reasoning_tokens)
    };

    // Final newline.
    if content.is_some() || reasoning.is_some() {
        println!();
    }

    Ok(AssistantMessage {
        content,
        reasoning,
        tool_calls,
    })
}

async fn handle_tool_calls(
    ctx: &Ctx,
    tool_calls: Vec<ToolCallRequest>,
) -> Result<Vec<ToolCallResult>> {
    let mut results = vec![];
    for call in tool_calls {
        results.push(handle_tool_call(ctx, call).await?);
    }

    Ok(results)
}

async fn handle_tool_call(ctx: &Ctx, call: ToolCallRequest) -> Result<ToolCallResult> {
    let result = ctx.mcp_client.call_tool(&call.name, call.arguments).await?;

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

struct Line {
    content: String,
    variant: LineVariant,
}

#[derive(Debug)]
enum LineVariant {
    Normal,
    Code,
    FencedCodeBlockStart { language: Option<String> },
    FencedCodeBlockEnd { indent: usize },
}

impl Line {
    fn new(content: String, in_fenced_code_block: bool) -> Self {
        let variant = if in_fenced_code_block && content.trim().ends_with("```") {
            let indent = content.chars().take_while(|c| c.is_whitespace()).count();

            LineVariant::FencedCodeBlockEnd { indent }
        } else if content.trim_start().starts_with("```") {
            let language = content
                .trim_start()
                .chars()
                .skip(3)
                .take_while(|c| c.is_alphanumeric())
                .collect::<String>();
            let language = if language.is_empty() {
                None
            } else {
                Some(language)
            };

            LineVariant::FencedCodeBlockStart { language }
        } else if in_fenced_code_block {
            LineVariant::Code
        } else {
            LineVariant::Normal
        };

        Line { content, variant }
    }
}

#[derive(Debug, Default)]
struct ResponseHandler {
    // The streamed, unprocessed lines received from the LLM.
    streamed: Vec<String>,
    // The lines that have been printed so far.
    printed: Vec<String>,
    buffer: String,
    in_fenced_code_block: bool,
    // (language, code)
    code_buffer: (Option<String>, Vec<String>),
    code_line: usize,

    // The last index of the line that ends a code block.
    // (streamed, printed)
    last_fenced_code_block_end: (usize, usize),
}

impl ResponseHandler {
    fn handle_stream(&mut self, data: &str, ctx: &Ctx) -> Result<()> {
        self.buffer.push_str(data);

        while let Some(Line { content, variant }) = self.get_line() {
            self.streamed.push(content);

            let lines = self.handle_line(&variant, ctx)?;
            stdout::typewriter(&lines, TYPEWRITER_DELAY)?;
            self.printed.extend(lines);
        }

        Ok(())
    }

    #[expect(clippy::too_many_lines)]
    fn handle_line(&mut self, variant: &LineVariant, ctx: &Ctx) -> Result<Vec<String>> {
        let Some(content) = self.streamed.last().map(String::as_str) else {
            return Ok(vec![]);
        };

        match variant {
            LineVariant::Code => {
                self.code_line += 1;
                self.code_buffer.1.push(content.to_owned());

                let mut buf = String::new();
                let config = code::Config {
                    language: self.code_buffer.0.clone(),
                    theme: ctx
                        .config
                        .style
                        .code
                        .color
                        .then(|| ctx.config.style.code.theme.clone()),
                };

                if !code::format(content, &mut buf, &config)? {
                    let config = code::Config {
                        language: None,
                        theme: config.theme,
                    };

                    code::format(content, &mut buf, &config)?;
                }

                if ctx.config.style.code.line_numbers {
                    buf.insert_str(
                        0,
                        &format!("{:2} │ ", self.code_line)
                            .with(Color::AnsiValue(238))
                            .to_string(),
                    );
                }

                Ok(vec![buf])
            }
            LineVariant::FencedCodeBlockStart { language } => {
                self.code_buffer.0.clone_from(language);
                self.code_buffer.1.clear();
                self.code_line = 0;
                self.in_fenced_code_block = true;

                Ok(vec![content.with(Color::AnsiValue(238)).to_string()])
            }
            LineVariant::FencedCodeBlockEnd { indent } => {
                self.last_fenced_code_block_end = (self.streamed.len(), self.printed.len() + 2);

                let path = self.persist_code_block()?;
                let mut links = vec![];

                match ctx.config.style.code.file_link {
                    LinkStyle::Off => {}
                    LinkStyle::Full => {
                        links.push(format!(
                            "{}see: file://{}",
                            " ".repeat(*indent),
                            path.display()
                        ));
                    }
                    LinkStyle::Osc8 => {
                        links.push(format!(
                            "{}[{}]",
                            " ".repeat(*indent),
                            hyperlink(
                                format!("file://{}", path.display()),
                                "open in editor".red().to_string()
                            )
                        ));
                    }
                }

                match ctx.config.style.code.copy_link {
                    LinkStyle::Off => {}
                    LinkStyle::Full => {
                        links.push(format!(
                            "{}copy: copy://{}",
                            " ".repeat(*indent),
                            path.display()
                        ));
                    }
                    LinkStyle::Osc8 => {
                        links.push(format!(
                            "{}[{}]",
                            " ".repeat(*indent),
                            hyperlink(
                                format!("copy://{}", path.display()),
                                "copy to clipboard".red().to_string()
                            )
                        ));
                    }
                }

                self.in_fenced_code_block = false;

                let mut lines = vec![content.with(Color::AnsiValue(238)).to_string()];
                if !links.is_empty() {
                    lines.push(links.join(" "));
                }

                Ok(lines)
            }
            LineVariant::Normal => {
                // We feed all the lines for markdown formatting, but only
                // print the last one, as the others are already printed.
                //
                // This helps the parser to use previous context to apply
                // the correct formatting to the current line.
                //
                // We only care about the lines after the last code block
                // end, because a) formatting context is reset after a code
                // block, and b) we dot not limit the line length of code, makes
                // it impossible to correctly find the non-printed lines based
                // on wrapped vs non-wrapped lines.
                let lines = self
                    .streamed
                    .iter()
                    .skip(self.last_fenced_code_block_end.0)
                    .cloned()
                    .collect::<Vec<_>>();

                // `termimad` removes empty lines at the start or end, but we
                // want to keep them as we will have more lines to print.
                let has_empty_line_start = lines.first().is_some_and(String::is_empty);
                let has_empty_line_end = lines.last().is_some_and(String::is_empty);

                let options = comrak::Options {
                    render: comrak::RenderOptions {
                        unsafe_: true,
                        prefer_fenced: true,
                        experimental_minimize_commonmark: true,
                        ..Default::default()
                    },
                    ..Default::default()
                };

                let formatted = comrak::markdown_to_commonmark(&lines.join("\n"), &options);

                let mut formatted =
                    FmtText::from(&termimad::MadSkin::default(), &formatted, Some(100)).to_string();

                if has_empty_line_start {
                    formatted.insert(0, '\n');
                }

                // Only add an extra newlien if we have more than one line,
                // otherwise a single empty line will be interpreted as both a
                // missing start and end newline.
                if has_empty_line_end && lines.len() > 1 {
                    formatted.push('\n');
                }

                let lines = formatted
                    .lines()
                    .skip(self.printed.len() - self.last_fenced_code_block_end.1)
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>();

                Ok(lines)
            }
        }
    }

    fn get_line(&mut self) -> Option<Line> {
        let s = &mut self.buffer;
        let idx = s.find('\n')?;

        // Determine the end index of the actual line *content*.
        // Check if the character before '\n' is '\r'.
        let end_idx = if idx > 0 && s.as_bytes().get(idx - 1) == Some(&b'\r') {
            idx - 1
        } else {
            idx
        };

        // Extract the line content *before* draining.
        // Creating a slice and then converting to owned String.
        let extracted_line = s[..end_idx].to_string();

        // Calculate the index *after* the newline sequence to drain up to.
        // This ensures we remove the '\n' and potentially the preceding '\r'.
        let drain_end_idx = idx + 1;
        s.drain(..drain_end_idx);

        Some(Line::new(extracted_line, self.in_fenced_code_block))
    }

    fn persist_code_block(&self) -> Result<PathBuf> {
        let code = self.code_buffer.1.clone();
        let language = self.code_buffer.0.as_deref().unwrap_or("txt");
        let ext = match language {
            "c++" => "cpp",
            "javascript" => "js",
            "python" => "py",
            "ruby" => "rb",
            "rust" => "rs",
            "typescript" => "ts",
            lang => lang,
        };

        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_millis();
        let path = std::env::temp_dir().join(format!("code_{millis}.{ext}"));

        fs::write(&path, code.join("\n"))?;

        Ok(path)
    }
}
