use std::{env, fmt::Write, fs, time};

use camino::Utf8Path;
use crossterm::style::Stylize as _;
use indexmap::{IndexMap, IndexSet};
use jp_config::{
    AppConfig,
    conversation::tool::{
        QuestionTarget, ToolConfigWithDefaults,
        style::{InlineResults, LinkStyle, ParametersStyle, TruncateLines},
    },
    style::{
        StyleConfig,
        reasoning::{ReasoningDisplayConfig, TruncateChars},
    },
};
use jp_conversation::{
    self,
    event::{ChatResponse, ToolCallRequest, ToolCallResponse},
};
use jp_llm::{ToolError, tool::ToolDefinition};
use jp_printer::PrinterWriter;
use jp_term::osc::hyperlink;
use jp_tool::{AnswerType, Question};
use serde_json::{Value, json};

use super::{ResponseHandler, turn::TurnState};
use crate::Error;

#[derive(Debug, Default, PartialEq)]
pub(super) struct StreamEventHandler {
    pub reasoning_tokens: String,
    pub content_tokens: String,
    pub tool_calls: Vec<ToolCallRequest>,
    pub tool_call_responses: Vec<ToolCallResponse>,
}

impl StreamEventHandler {
    pub(super) fn handle_chat_chunk(
        &mut self,
        reasoning_display: ReasoningDisplayConfig,
        chunk: ChatResponse,
    ) -> Option<String> {
        match chunk {
            ChatResponse::Reasoning { ref reasoning } if !reasoning.is_empty() => {
                let mut display = match reasoning_display {
                    ReasoningDisplayConfig::Summary => todo!(),
                    ReasoningDisplayConfig::Static | ReasoningDisplayConfig::Progress
                        if self.reasoning_tokens.is_empty() =>
                    {
                        Some("reasoning...".to_owned())
                    }
                    // For progress display, we start with `reasoning...` and
                    // then append a dot for each new reasoning chunk, to
                    // indicate that the reasoning is still ongoing.
                    ReasoningDisplayConfig::Progress => Some(".".to_owned()),
                    ReasoningDisplayConfig::Full => Some(reasoning.clone()),
                    ReasoningDisplayConfig::Truncate(TruncateChars { characters }) => {
                        let remaining =
                            characters.saturating_sub(self.reasoning_tokens.chars().count());

                        if remaining > 0 {
                            let mut data: String = reasoning.chars().take(remaining).collect();
                            if data.chars().count() == remaining {
                                data.push_str("...");
                            }

                            Some(data)
                        } else {
                            None
                        }
                    }
                    _ => None,
                };

                if matches!(
                    reasoning_display,
                    ReasoningDisplayConfig::Full | ReasoningDisplayConfig::Truncate(_)
                ) && let Some(v) = display.as_mut()
                {
                    if self.reasoning_tokens.is_empty() {
                        v.insert_str(0, "> ");
                    }

                    *v = v.replace('\n', "\n> ");
                }

                self.reasoning_tokens.push_str(reasoning);
                display
            }

            ChatResponse::Message { mut message } if !message.is_empty() => {
                let reasoning_ended =
                    !self.reasoning_tokens.is_empty() && self.content_tokens.is_empty();

                self.content_tokens.push_str(&message);

                // If the response includes reasoning, we add two newlines
                // after the reasoning, but before the content.
                if !matches!(reasoning_display, ReasoningDisplayConfig::Hidden) && reasoning_ended {
                    message = format!("\n\n---\n\n{message}");
                }

                Some(message)
            }
            _ => None,
        }
    }

    #[allow(clippy::too_many_lines)]
    pub async fn handle_tool_call(
        &mut self,
        cfg: &AppConfig,
        mcp_client: &jp_mcp::Client,
        root: &Utf8Path,
        is_tty: bool,
        turn_state: &mut TurnState,
        call: ToolCallRequest,
        handler: &mut ResponseHandler,
        mut writer: PrinterWriter<'_>,
    ) -> Result<Option<String>, Error> {
        let Some(tool_config) = cfg.conversation.tools.get(&call.name) else {
            let response = ToolCallResponse {
                id: call.id.clone(),
                result: Err(format!("Tool '{}' not found.", call.name)),
            };

            self.tool_call_responses.push(response.clone());
            return Ok(None);
        };

        let mut arguments_without_tool_answers = call.arguments.clone();

        // Remove the special `tool_answers` argument, if any.
        let mut tool_answers = arguments_without_tool_answers
            .remove("tool_answers")
            .map_or(Ok(IndexMap::new()), |v| match v {
                Value::Object(v) => Ok(v.into_iter().collect()),
                _ => Err(ToolError::ToolCallFailed(
                    "`tool_answers` argument must be an object".to_owned(),
                )),
            })?;

        // Remove any pending questions for this tool call that are now
        // answered.
        let mut answered_questions = false;
        if let Some(pending) = turn_state.pending_tool_call_questions.get_mut(&call.name) {
            for question_id in tool_answers.keys() {
                answered_questions = pending.shift_remove(question_id);
            }

            if pending.is_empty() {
                turn_state
                    .pending_tool_call_questions
                    .shift_remove(&call.name);
            }
        }

        let editor = cfg.editor.path();

        self.tool_calls.push(call.clone());
        let tool = ToolDefinition::new(
            &call.name,
            tool_config.source(),
            tool_config.description().map(str::to_owned),
            tool_config.parameters().clone(),
            tool_config.questions(),
            mcp_client,
        )
        .await?;

        if handler.render_tool_calls && !answered_questions {
            let (_raw, args) = match &tool_config.style().parameters {
                ParametersStyle::Off => (false, ".".to_owned()),
                ParametersStyle::Json => {
                    let args = serde_json::to_string_pretty(&call.arguments)
                        .unwrap_or_else(|_| format!("{:#}", Value::Object(call.arguments.clone())));

                    (false, format!(" with arguments:\n\n```json\n{args}\n```"))
                }
                ParametersStyle::FunctionCall => {
                    let mut buf = String::new();
                    buf.push('(');
                    for (i, (key, value)) in call.arguments.iter().enumerate() {
                        if i > 0 {
                            buf.push_str(", ");
                        }
                        buf.push_str(&format!("{key}={value}"));
                    }
                    buf.push(')');
                    (false, buf)
                }
                ParametersStyle::Custom(command) => {
                    let cmd = command.clone().command();
                    let name = tool_config.source().tool_name();

                    match tool.format_args(name, &cmd, &arguments_without_tool_answers, root)? {
                        Ok(args) if args.is_empty() => (false, ".".to_owned()),
                        Ok(args) => (true, format!(":\n\n{args}")),
                        result @ Err(_) => {
                            let response = ToolCallResponse {
                                id: call.id,
                                result,
                            };

                            self.tool_call_responses.push(response.clone());
                            return Ok(None);
                        }
                    }
                }
            };

            write!(writer, "\n\nCalling tool **{}**", tool.name)?;
            write!(writer, "{args}")?;
            write!(writer, "\n\n")?;
        }

        loop {
            match tool
                .call(
                    call.id.clone(),
                    Value::Object(arguments_without_tool_answers.clone()),
                    &tool_answers,
                    turn_state
                        .pending_tool_call_questions
                        .get(&call.name)
                        .unwrap_or(&IndexSet::new()),
                    mcp_client,
                    tool_config.clone(),
                    root,
                    editor.as_deref(),
                    writer,
                )
                .await
            {
                Ok(result) => {
                    self.tool_call_responses.push(result.clone());
                    return build_tool_call_response(
                        &cfg.style,
                        &result,
                        &tool_config,
                        handler,
                        writer,
                    );
                }
                Err(ToolError::Skipped { reason }) => {
                    self.tool_call_responses.push(ToolCallResponse {
                        id: call.id.clone(),
                        result: {
                            let mut msg = "Tool execution skipped by user.".to_string();
                            if let Some(reason) = reason {
                                msg.push_str(&format!("\n\n{reason}"));
                            }
                            Ok(msg)
                        },
                    });

                    return Ok(None);
                }
                Err(ToolError::NeedsInput { question }) => {
                    // Check answers in priority order:
                    // 1. Turn-level persisted answers
                    // 2. Config-level automated answers
                    // 3. Interactive prompt (or default)
                    let answer = if let Some(answer) = turn_state
                        .persisted_tool_answers
                        .get(&call.name)
                        .and_then(|tool_answers| tool_answers.get(&question.id))
                    {
                        answer.clone()
                    } else if let Some(answer) = tool_config.get_answer(&question.id) {
                        answer.clone()
                    } else if matches!(
                        tool_config.question_target(&question.id),
                        Some(QuestionTarget::Assistant)
                    ) {
                        // Keep track of pending questions for this tool call.
                        turn_state
                            .pending_tool_call_questions
                            .entry(call.name.clone())
                            .or_default()
                            .insert(question.id.clone());

                        // Ask the assistant to answer the question
                        let mut args = call.arguments.clone();
                        args.entry("tool_answers".to_owned())
                            .and_modify(|v| match v {
                                Value::Object(_) => {}
                                _ => *v = json!({}),
                            })
                            .or_insert_with(|| json!({}))
                            .as_object_mut()
                            .expect("tool_answers must be an object")
                            .insert(question.id.clone(), "<ANSWER HERE>".into());

                        let values = match question.answer_type {
                            AnswerType::Boolean => "any boolean type (true or false).".to_owned(),
                            AnswerType::Select { options } => indoc::formatdoc! {"
                                one of the following string values:

                                - {}
                            ", options.join("\n- ")},
                            AnswerType::Text => "any string.".to_owned(),
                        };

                        let response = ToolCallResponse {
                            id: call.id.clone(),
                            result: Ok(indoc::formatdoc! {"
                                Tool requires additional input before it can complete the request:

                                {}

                                Please re-run the tool with the following arguments:

                                ```json
                                {}
                                ```

                                Where `<ANSWER HERE>` must be {values}
                            ",
                            question.text,
                            serde_json::to_string_pretty(&args)?}),
                        };

                        self.tool_call_responses.push(response.clone());

                        return Ok(None);
                    } else if is_tty {
                        let (answer, persist_level) = prompt_user(&question, writer)?;

                        // Store turn-level answers for reuse across tool calls
                        if persist_level == jp_tool::PersistLevel::Turn {
                            turn_state
                                .persisted_tool_answers
                                .entry(call.name.clone())
                                .or_default()
                                .insert(question.id.clone(), answer.clone());
                        }

                        answer
                    } else {
                        question.default.unwrap_or_default()
                    };

                    tool_answers.insert(question.id.clone(), answer);
                }
                Err(e) => return Err(e.into()),
            }
        }
    }
}

fn prompt_user(
    question: &Question,
    mut writer: PrinterWriter<'_>,
) -> Result<(Value, jp_tool::PersistLevel), Error> {
    match &question.answer_type {
        AnswerType::Boolean => prompt_boolean_git_style(question, writer),
        AnswerType::Select { options } => {
            let answer: Value = inquire::Select::new(&question.text, options.clone())
                .prompt_with_writer(&mut writer)
                .map(Into::into)
                .map_err(|e: inquire::error::InquireError| Error::from(e))?;
            Ok((answer, jp_tool::PersistLevel::None))
        }
        AnswerType::Text => {
            let mut inquiry = inquire::Text::new(&question.text);

            if let Some(default) = question.default.as_ref().and_then(Value::as_str) {
                inquiry = inquiry.with_default(default);
            }

            let answer: Value = inquiry
                .prompt_with_writer(&mut writer)
                .map(Into::into)
                .map_err(|e: inquire::error::InquireError| Error::from(e))?;
            Ok((answer, jp_tool::PersistLevel::None))
        }
    }
}

fn prompt_boolean_git_style(
    question: &Question,
    mut writer: PrinterWriter<'_>,
) -> Result<(Value, jp_tool::PersistLevel), Error> {
    use jp_inquire::{InlineOption, InlineSelect};

    let options = vec![
        InlineOption::new('y', "yes, just this once"),
        InlineOption::new('Y', "yes, and remember for this turn"),
        InlineOption::new('n', "no, just this once"),
        InlineOption::new('N', "no, and remember for this turn"),
    ];

    let select = InlineSelect::new(&question.text, options);
    let answer = select.prompt(&mut writer)?;

    match answer {
        'y' => Ok((Value::Bool(true), jp_tool::PersistLevel::None)),
        'Y' => Ok((Value::Bool(true), jp_tool::PersistLevel::Turn)),
        'n' => Ok((Value::Bool(false), jp_tool::PersistLevel::None)),
        'N' => Ok((Value::Bool(false), jp_tool::PersistLevel::Turn)),
        _ => unreachable!(),
    }
}

fn build_tool_call_response(
    _style: &StyleConfig,
    response: &ToolCallResponse,
    tool_config: &ToolConfigWithDefaults,
    handler: &mut ResponseHandler,
    mut writer: PrinterWriter<'_>,
) -> Result<Option<String>, Error> {
    let content = if let Ok(json) = serde_json::from_str::<Value>(response.content().trim()) {
        format!("```json\n{}\n```", serde_json::to_string_pretty(&json)?)
    } else {
        response.content().trim().to_owned()
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
        if content.trim().starts_with('<') && quick_xml::de::from_str::<Value>(&content).is_ok() {
            ext = Some("xml".to_owned());
        } else if content.trim().starts_with('{') && serde_json::from_str::<Value>(&content).is_ok()
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
        InlineResults::Truncate(TruncateLines { lines }) => lines,
        _ => content.lines().count(),
    };

    if handler.render_tool_calls
        && !matches!(tool_config.style().inline_results, InlineResults::Off)
    {
        let mut intro = "\nTool call result".to_owned();
        match tool_config.style().inline_results {
            InlineResults::Truncate(TruncateLines { lines }) if lines < content.lines().count() => {
                intro.push_str(&format!(" _(truncated to {lines} lines)_"));
            }
            _ => {}
        }
        intro.push_str(":\n");

        write!(&mut writer, "{intro}")?;
    }

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
        if !data.is_empty() && !data.ends_with('\n') {
            data.push('\n');
        }

        write!(writer, "{data}")?;
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
        write!(writer, "{link}")?;
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;

    use super::*;

    #[test]
    #[expect(clippy::too_many_lines)]
    fn test_stream_event_handler_handle_chat_chunk() {
        struct TestCase {
            handler: StreamEventHandler,
            chunk: ChatResponse,
            show_reasoning: bool,
            output: Option<String>,
            mutated_handler: StreamEventHandler,
        }

        let cases = IndexMap::from([
            ("empty content chunk", TestCase {
                handler: StreamEventHandler::default(),
                chunk: ChatResponse::message(""),
                show_reasoning: true,
                output: None,
                mutated_handler: StreamEventHandler::default(),
            }),
            ("empty reasoning chunk", TestCase {
                handler: StreamEventHandler::default(),
                chunk: ChatResponse::reasoning(""),
                show_reasoning: true,
                output: None,
                mutated_handler: StreamEventHandler::default(),
            }),
            ("reasoning chunk with show_reasoning=true", TestCase {
                handler: StreamEventHandler::default(),
                chunk: ChatResponse::reasoning("Let me think..."),
                show_reasoning: true,
                output: Some("> Let me think...".into()),
                mutated_handler: StreamEventHandler {
                    reasoning_tokens: "Let me think...".into(),
                    ..Default::default()
                },
            }),
            ("reasoning chunk with show_reasoning=false", TestCase {
                handler: StreamEventHandler::default(),
                chunk: ChatResponse::reasoning("Let me think..."),
                show_reasoning: false,
                output: None,
                mutated_handler: StreamEventHandler {
                    reasoning_tokens: "Let me think...".into(),
                    ..Default::default()
                },
            }),
            ("content after reasoning adds separator", TestCase {
                handler: StreamEventHandler {
                    reasoning_tokens: "I reasoned".into(),
                    ..Default::default()
                },
                chunk: ChatResponse::message("Answer"),
                show_reasoning: true,
                output: Some("\n\n---\n\nAnswer".into()),
                mutated_handler: StreamEventHandler {
                    reasoning_tokens: "I reasoned".into(),
                    content_tokens: "Answer".into(),
                    ..Default::default()
                },
            }),
            ("content after reasoning without show_reasoning", TestCase {
                handler: StreamEventHandler {
                    reasoning_tokens: "I reasoned".into(),
                    ..Default::default()
                },
                chunk: ChatResponse::message("Answer"),
                show_reasoning: false,
                output: Some("Answer".into()),
                mutated_handler: StreamEventHandler {
                    reasoning_tokens: "I reasoned".into(),
                    content_tokens: "Answer".into(),
                    ..Default::default()
                },
            }),
            ("subsequent content chunks accumulate", TestCase {
                handler: StreamEventHandler {
                    content_tokens: "Hello".into(),
                    ..Default::default()
                },
                chunk: ChatResponse::message(" world"),
                show_reasoning: false,
                output: Some(" world".into()),
                mutated_handler: StreamEventHandler {
                    content_tokens: "Hello world".into(),
                    ..Default::default()
                },
            }),
        ]);

        for (
            name,
            TestCase {
                mut handler,
                chunk,
                show_reasoning,
                output,
                mutated_handler,
            },
        ) in cases
        {
            let reasoning_display = if show_reasoning {
                ReasoningDisplayConfig::Full
            } else {
                ReasoningDisplayConfig::Hidden
            };

            let result = handler.handle_chat_chunk(reasoning_display, chunk);
            assert_eq!(result, output, "Failed test case: {name}");
            assert_eq!(handler, mutated_handler, "Failed test case: {name}");
        }
    }
}
