use std::{path::Path, sync::Arc};

use crossterm::style::Stylize as _;
use indexmap::IndexMap;
use jp_config::conversation::tool::{
    OneOrManyTypes, ResultMode, RunMode, ToolCommandConfig, ToolConfigWithDefaults,
    ToolParameterConfig, ToolSource, item::ToolParameterItemConfig,
};
use jp_conversation::event::ToolCallResponse;
use jp_inquire::{InlineOption, InlineSelect};
use jp_mcp::{
    RawContent, ResourceContents,
    id::{McpServerId, McpToolId},
};
use jp_tool::Outcome;
use minijinja::Environment;
use serde_json::{Map, Value, json};
use tracing::{info, trace};

use crate::error::ToolError;

/// The definition of a tool.
///
/// The definition source is either a [`ToolConfig`] for `local` tools, or a
/// combination of `ToolConfig` and MCP server information for `mcp` tools, or
/// hard-coded for definitions `builtin` tools.
///
/// [`ToolConfig`]: jp_config::conversation::tool::ToolConfig
#[derive(Debug, Clone)]
pub struct ToolDefinition {
    pub name: String,
    pub description: Option<String>,
    pub parameters: IndexMap<String, ToolParameterConfig>,
}

impl ToolDefinition {
    pub async fn new(
        name: &str,
        source: &ToolSource,
        description: Option<String>,
        parameters: IndexMap<String, ToolParameterConfig>,
        mcp_client: &jp_mcp::Client,
    ) -> Result<Self, ToolError> {
        match &source {
            ToolSource::Local { .. } => Ok(local_tool_definition(
                name.to_owned(),
                description,
                parameters,
            )),
            ToolSource::Mcp { server, tool } => {
                mcp_tool_definition(
                    server.as_ref(),
                    name,
                    tool.as_deref(),
                    description,
                    parameters,
                    mcp_client,
                )
                .await
            }
            ToolSource::Builtin { .. } => todo!(),
        }
    }

    pub fn format_args(
        &self,
        name: Option<&str>,
        cmd: &ToolCommandConfig,
        arguments: &Map<String, Value>,
        root: &Path,
    ) -> Result<Result<String, String>, ToolError> {
        let name = name.unwrap_or(&self.name);
        if arguments.is_empty() {
            return Ok(Ok(String::new()));
        }

        let ctx = json!({
            "tool": {
                "name": self.name,
                "arguments": arguments,
            },
            "context": {
                "format_parameters": true,
                "root": root.to_string_lossy(),
            },
        });

        run_cmd_with_ctx(name, cmd, &ctx, root)
    }

    pub async fn call(
        &self,
        id: String,
        mut arguments: Value,
        answers: &IndexMap<String, Value>,
        mcp_client: &jp_mcp::Client,
        config: ToolConfigWithDefaults,
        root: &Path,
        editor: Option<&Path>,
        writer: &mut dyn Write,
    ) -> Result<ToolCallResponse, ToolError> {
        info!(tool = %self.name, arguments = ?arguments, "Calling tool.");

        // If the tool call has answers to provide to the tool, it means the
        // tool already ran once, and we should not ask for confirmation again.
        let run_mode = if answers.is_empty() {
            config.run()
        } else {
            RunMode::Unattended
        };

        let mut result_mode = config.result();
        if answers.is_empty() {
            self.prepare_run(
                run_mode,
                &mut result_mode,
                &mut arguments,
                config.source(),
                mcp_client,
                editor,
                writer,
            )
            .await?;
        }

        let result = match config.source() {
            ToolSource::Local { tool } => {
                self.call_local(id, &arguments, answers, &config, tool.as_deref(), root)?
            }
            ToolSource::Mcp { server, tool } => {
                self.call_mcp(
                    id,
                    &arguments,
                    mcp_client,
                    server.as_deref(),
                    tool.as_deref(),
                )
                .await?
            }
            ToolSource::Builtin { .. } => todo!(),
        };

        trace!(result = ?result, "Tool call completed.");
        self.prepare_result(result, result_mode, editor, writer)
    }

    fn call_local(
        &self,
        id: String,
        arguments: &Value,
        answers: &IndexMap<String, Value>,
        config: &ToolConfigWithDefaults,
        tool: Option<&str>,
        root: &Path,
    ) -> Result<ToolCallResponse, ToolError> {
        let name = tool.unwrap_or(&self.name);

        // TODO: Should we enforce at a type-level this for all tool calls, even
        // MCP?
        if let Some(args) = arguments.as_object()
            && let Err(error) = validate_tool_arguments(
                args,
                &config
                    .parameters()
                    .iter()
                    .map(|(k, v)| (k.to_owned(), v.required))
                    .collect(),
            )
        {
            return Ok(ToolCallResponse {
                id,
                result: Err(format!("Invalid arguments: {error}")),
            });
        }

        let ctx = json!({
            "tool": {
                "name": name,
                "arguments": arguments,
                "answers": answers,
            },
            "context": {
                "root": root.to_string_lossy().into_owned(),
            },
        });

        let Some(command) = config.command() else {
            return Err(ToolError::MissingCommand);
        };

        Ok(ToolCallResponse {
            id,
            result: run_cmd_with_ctx(name, &command, &ctx, root)?,
        })
    }

    async fn call_mcp(
        &self,
        id: String,
        arguments: &Value,
        mcp_client: &jp_mcp::Client,
        server: Option<&str>,
        tool: Option<&str>,
    ) -> Result<ToolCallResponse, ToolError> {
        let name = tool.unwrap_or(&self.name);

        let result = mcp_client
            .call_tool(name, server, arguments)
            .await
            .map_err(ToolError::McpRunToolError)?;

        let content = result
            .content
            .into_iter()
            .filter_map(|v| match v.raw {
                RawContent::Text(v) => Some(v.text),
                RawContent::Resource(v) => match v.resource {
                    ResourceContents::TextResourceContents { text, .. } => Some(text),
                    ResourceContents::BlobResourceContents { blob, .. } => Some(blob),
                },
                RawContent::Image(_) | RawContent::Audio(_) => None,
            })
            .collect::<Vec<_>>()
            .join("\n\n");

        Ok(ToolCallResponse {
            id,
            result: if result.is_error.unwrap_or_default() {
                Err(content)
            } else {
                Ok(content)
            },
        })
    }

    /// Return a map of parameter names to JSON schemas.
    #[must_use]
    pub fn to_parameters_map(&self) -> Map<String, Value> {
        self.parameters
            .clone()
            .into_iter()
            .map(|(k, v)| (k, v.to_json_schema()))
            .collect::<Map<_, _>>()
    }

    /// Return a JSON schema for the parameters of the tool.
    #[must_use]
    pub fn to_parameters_schema(&self) -> Value {
        let required = self
            .parameters
            .iter()
            .filter(|(_, cfg)| cfg.required)
            .map(|(k, _)| k.clone())
            .collect::<Vec<_>>();

        serde_json::json!({
            "type": "object",
            "properties": self.to_parameters_map(),
            "additionalProperties": false,
            "required": required,
        })
    }

    #[expect(clippy::too_many_lines)]
    async fn prepare_run(
        &self,
        run_mode: RunMode,
        result_mode: &mut ResultMode,
        arguments: &mut Value,
        source: &ToolSource,
        mcp_client: &jp_mcp::Client,
        editor: Option<&Path>,
        writer: &mut dyn Write,
    ) -> Result<(), ToolError> {
        match run_mode {
            RunMode::Ask => match InlineSelect::new(
                {
                    let mut question = format!(
                        "Run {} {} tool",
                        match source {
                            ToolSource::Builtin { .. } => "built-in",
                            ToolSource::Local { .. } => "local",
                            ToolSource::Mcp { .. } => "mcp",
                        },
                        self.name.as_str().bold().yellow(),
                    );

                    if let ToolSource::Mcp { server, tool } = source {
                        let tool = McpToolId::new(tool.as_ref().unwrap_or(&self.name));
                        let server = server.as_ref().map(|s| McpServerId::new(s.clone()));
                        let server_id = mcp_client
                            .get_tool_server_id(&tool, server.as_ref())
                            .await
                            .map_err(ToolError::McpGetToolError)?;

                        question = format!(
                            "{} from {} server?",
                            question,
                            server_id.as_str().bold().blue()
                        );
                    }

                    question
                },
                vec![
                    InlineOption::new('y', "Run tool"),
                    InlineOption::new('n', "Skip running tool"),
                    InlineOption::new(
                        'r',
                        format!(
                            "Change run mode (current: {})",
                            run_mode.to_string().italic().yellow()
                        ),
                    ),
                    InlineOption::new(
                        'x',
                        format!(
                            "Change result mode (current: {})",
                            result_mode.to_string().italic().yellow()
                        ),
                    ),
                    InlineOption::new('p', "Print raw tool arguments"),
                ],
            )
            .prompt(writer)
            .unwrap_or('n')
            {
                'y' => return Ok(()),
                'n' => return Err(ToolError::Skipped { reason: None }),
                'r' => {
                    let new_run_mode = match InlineSelect::new("Run Mode", {
                        let mut options = vec![
                            InlineOption::new('a', "Ask"),
                            InlineOption::new('u', "Unattended (Run Tool Without Changes)"),
                            InlineOption::new('e', "Edit Arguments"),
                            InlineOption::new('s', "Skip Call"),
                        ];

                        if editor.is_some() {
                            options.push(InlineOption::new('S', "Skip Call, with reasoning"));
                        }

                        options.push(InlineOption::new('c', "Keep Current Run Mode"));
                        options
                    })
                    .prompt(writer)
                    .unwrap_or('c')
                    {
                        'a' => RunMode::Ask,
                        'u' => RunMode::Unattended,
                        'e' => RunMode::Edit,
                        's' => RunMode::Skip,
                        'S' => match editor {
                            None => RunMode::Skip,
                            Some(editor) => {
                                return Err(ToolError::Skipped {
                                    reason: Some(
                                        open_editor::EditorCallBuilder::new()
                                            .with_editor(open_editor::Editor::from_bin_path(
                                                editor.to_path_buf(),
                                            ))
                                            .edit_string(
                                                "_Provide reasoning for skipping tool execution_",
                                            )
                                            .map_err(|error| ToolError::OpenEditorError {
                                                arguments: arguments.clone(),
                                                error,
                                            })?,
                                    ),
                                });
                            }
                        },
                        'c' => run_mode,
                        _ => unimplemented!(),
                    };

                    return Box::pin(self.prepare_run(
                        new_run_mode,
                        result_mode,
                        arguments,
                        source,
                        mcp_client,
                        editor,
                        writer,
                    ))
                    .await;
                }
                'x' => {
                    match InlineSelect::new("Result Mode", vec![
                        InlineOption::new('a', "Ask"),
                        InlineOption::new('u', "Unattended (Delver Payload As Is)"),
                        InlineOption::new('e', "Edit Result Payload"),
                        InlineOption::new('s', "Skip (Don't Deliver Payload)"),
                        InlineOption::new('c', "Keep Current Result Mode"),
                    ])
                    .prompt(writer)
                    .unwrap_or('c')
                    {
                        'a' => *result_mode = ResultMode::Ask,
                        'u' => *result_mode = ResultMode::Unattended,
                        'e' => *result_mode = ResultMode::Edit,
                        's' => *result_mode = ResultMode::Skip,
                        'c' => {}
                        _ => unimplemented!(),
                    }

                    return Box::pin(self.prepare_run(
                        run_mode,
                        result_mode,
                        arguments,
                        source,
                        mcp_client,
                        editor,
                        writer,
                    ))
                    .await;
                }
                'p' => {
                    if let Err(error) =
                        writeln!(writer, "{}\n", serde_json::to_string_pretty(&arguments)?)
                    {
                        error!(%error, "Failed to write arguments");
                    }

                    return Box::pin(self.prepare_run(
                        RunMode::Ask,
                        result_mode,
                        arguments,
                        source,
                        mcp_client,
                        editor,
                        writer,
                    ))
                    .await;
                }
                _ => unreachable!(),
            },
            RunMode::Unattended => return Ok(()),
            RunMode::Skip => return Err(ToolError::Skipped { reason: None }),
            RunMode::Edit => {
                let mut args = serde_json::to_string_pretty(&arguments).map_err(|error| {
                    ToolError::SerializeArgumentsError {
                        arguments: arguments.clone(),
                        error,
                    }
                })?;

                *arguments = {
                    if let Some(editor) = editor {
                        open_editor::EditorCallBuilder::new()
                            .with_editor(open_editor::Editor::from_bin_path(editor.to_path_buf()))
                            .edit_string_mut(&mut args)
                            .map_err(|error| ToolError::OpenEditorError {
                                arguments: arguments.clone(),
                                error,
                            })?;
                    }

                    // If the user removed all data from the arguments, we consider the
                    // edit a no-op, and ask the user if they want to run the tool.
                    if args.trim().is_empty() {
                        return Box::pin(self.prepare_run(
                            RunMode::Ask,
                            result_mode,
                            arguments,
                            source,
                            mcp_client,
                            editor,
                            writer,
                        ))
                        .await;
                    }

                    match serde_json::from_str::<Value>(&args) {
                        Ok(value) => value,

                        // If we can't parse the arguments as valid JSON, we consider
                        // the input invalid, and ask the user if they want to re-open
                        // the editor.
                        Err(error) => {
                            if let Err(error) = writeln!(writer, "JSON parsing error: {error}") {
                                error!(%error, "Failed to write error");
                            }

                            let retry = InlineSelect::new("Re-open editor?", vec![
                                InlineOption::new('y', "Open editor to edit arguments"),
                                InlineOption::new('n', "Skip editing, failing with error"),
                            ])
                            .with_default('y')
                            .prompt(writer)
                            .unwrap_or('n');

                            if retry == 'n' {
                                return Err(ToolError::EditArgumentsError {
                                    arguments: arguments.clone(),
                                    error,
                                });
                            }

                            return Box::pin(self.prepare_run(
                                RunMode::Edit,
                                result_mode,
                                arguments,
                                source,
                                mcp_client,
                                editor,
                                writer,
                            ))
                            .await;
                        }
                    }
                };
            }
        }

        Ok(())
    }

    fn prepare_result(
        &self,
        mut result: ToolCallResponse,
        result_mode: ResultMode,
        editor: Option<&Path>,
        writer: &mut dyn Write,
    ) -> Result<ToolCallResponse, ToolError> {
        match result_mode {
            ResultMode::Ask => match InlineSelect::new(
                format!(
                    "Deliver the results of the {} tool call?",
                    self.name.as_str().bold().yellow(),
                ),
                vec![
                    InlineOption::new('y', "Deliver results"),
                    InlineOption::new('n', "Do not deliver results"),
                    InlineOption::new('e', "Edit results manually"),
                ],
            )
            .with_default('y')
            .prompt(writer)
            .unwrap_or('n')
            {
                'y' => return Ok(result),
                'n' => {
                    return Ok(ToolCallResponse {
                        id: result.id,
                        result: Ok("Tool call result omitted by user.".into()),
                    });
                }
                'e' => {}
                _ => unreachable!(),
            },
            ResultMode::Unattended => return Ok(result),
            ResultMode::Skip => {
                return Ok(ToolCallResponse {
                    id: result.id,
                    result: Ok("Tool ran successfully.".into()),
                });
            }
            ResultMode::Edit => {}
        }

        if let Some(editor) = editor {
            let content = open_editor::EditorCallBuilder::new()
                .with_editor(open_editor::Editor::from_bin_path(editor.to_path_buf()))
                .edit_string(result.content())
                .map_err(|error| ToolError::OpenEditorError {
                    arguments: Value::Null,
                    error,
                })?;

            // If the user removed all data from the result, we consider the edit a
            // no-op, and ask the user if they want to deliver the tool results.
            if content.trim().is_empty() {
                return self.prepare_result(result, ResultMode::Ask, Some(editor), writer);
            }

            result.result = Ok(content);
        }

        Ok(result)
    }
}

fn run_cmd_with_ctx(
    name: &str,
    command: &ToolCommandConfig,
    ctx: &Value,
    root: &Path,
) -> Result<Result<String, String>, ToolError> {
    let command = {
        let tmpl = Arc::new(Environment::new());

        let program =
            tmpl.render_str(&command.program, ctx)
                .map_err(|error| ToolError::TemplateError {
                    data: command.program.clone(),
                    error,
                })?;

        let args = command
            .args
            .iter()
            .map(|s| tmpl.render_str(s, ctx))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| ToolError::TemplateError {
                data: command.args.join(" ").clone(),
                error,
            })?;

        let expression = if command.shell {
            let cmd = std::iter::once(program.clone())
                .chain(args.iter().cloned())
                .collect::<Vec<_>>()
                .join(" ");

            duct_sh::sh_dangerous(cmd)
        } else {
            duct::cmd(program.clone(), args)
        };

        expression
            .dir(root)
            .unchecked()
            .stdout_capture()
            .stderr_capture()
    };

    match command.run() {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let content = match serde_json::from_str::<Outcome>(&stdout) {
                Err(_) => stdout.to_string(),
                Ok(Outcome::Error {
                    transient,
                    message,
                    trace,
                }) => {
                    if transient {
                        return Ok(Err(json!({
                            "message": message,
                            "trace": trace,
                        })
                        .to_string()));
                    }

                    return Err(ToolError::ToolCallFailed(stdout.to_string()));
                }
                Ok(Outcome::Success { content }) => content,
                Ok(Outcome::NeedsInput { question }) => {
                    return Err(ToolError::NeedsInput { question });
                }
            };

            if output.status.success() {
                Ok(Ok(content))
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Ok(Err(json!({
                    "message": format!("Tool '{name}' execution failed."),
                    "stderr": stderr,
                    "stdout": content,
                })
                .to_string()))
            }
        }
        Err(error) => Ok(Err(json!({
            "message": format!(
                "Failed to execute command '{command:?}': {error}",
            ),
        })
        .to_string())),
    }
}

fn validate_tool_arguments(
    arguments: &Map<String, Value>,
    parameters: &IndexMap<String, bool>,
) -> Result<(), ToolError> {
    let unknown = arguments
        .keys()
        .filter(|k| !parameters.contains_key(*k))
        .cloned()
        .collect::<Vec<_>>();

    let mut missing = vec![];
    for (name, required) in parameters {
        if *required && !arguments.contains_key(name) {
            missing.push(name.to_owned());
        }
    }

    if !missing.is_empty() || !unknown.is_empty() {
        return Err(ToolError::Arguments { missing, unknown });
    }

    Ok(())
}

pub async fn tool_definitions(
    configs: impl Iterator<Item = (&str, ToolConfigWithDefaults)>,
    mcp_client: &jp_mcp::Client,
) -> Result<Vec<ToolDefinition>, ToolError> {
    let mut definitions = vec![];
    for (name, config) in configs {
        // Skip disabled tools.
        if !config.enable() {
            continue;
        }

        definitions.push(
            ToolDefinition::new(
                name,
                config.source(),
                config.description().map(str::to_owned),
                config.parameters().clone(),
                mcp_client,
            )
            .await?,
        );
    }

    Ok(definitions)
}

fn local_tool_definition(
    name: String,
    description: Option<String>,
    parameters: IndexMap<String, ToolParameterConfig>,
) -> ToolDefinition {
    ToolDefinition {
        name,
        description,
        parameters,
    }
}

#[expect(clippy::too_many_lines)]
async fn mcp_tool_definition(
    server: Option<&String>,
    name: &str,
    source_name: Option<&str>,
    mut description: Option<String>,
    parameters: IndexMap<String, ToolParameterConfig>,
    mcp_client: &jp_mcp::Client,
) -> Result<ToolDefinition, ToolError> {
    let mcp_tool = {
        trace!(?server, tool = %name, "Fetching tool from MCP server");

        let server_id = server.as_ref().map(|s| McpServerId::new(s.to_owned()));
        mcp_client
            .get_tool(
                &McpToolId::new(source_name.unwrap_or(name)),
                server_id.as_ref(),
            )
            .await
            .map_err(ToolError::McpGetToolError)
    }?;

    match (description.as_mut(), mcp_tool.description) {
        (None, Some(mcp)) => {
            description = Some(mcp.to_string());
        }
        // TODO: should use `minijinja` instead.
        (Some(desc), Some(mcp)) => *desc = desc.replace("{{description}}", mcp.as_ref()),
        (Some(_) | None, None) => {}
    }

    let schema = mcp_tool.input_schema.as_ref().clone();
    let required_properties = schema
        .get("required")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str())
        .collect::<Vec<_>>();

    let mut params = IndexMap::new();
    for (name, opts) in schema
        .get("properties")
        .and_then(|v| v.as_object())
        .into_iter()
        .flatten()
    {
        let override_cfg = parameters.get(name.as_str());

        let kind = match override_cfg.map(|v| v.kind.clone()) {
            // Use `override` type if present.
            Some(kind) => kind,
            // Or use the type from the schema.
            None => match opts.get("type").unwrap_or(&Value::Null) {
                Value::String(v) => OneOrManyTypes::One(v.to_owned()),
                Value::Array(v) => OneOrManyTypes::Many(
                    v.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect(),
                ),
                value => {
                    if value.is_null()
                        && let Some(any) = opts
                            .get("anyOf")
                            .and_then(Value::as_array)
                            .map(|v| {
                                v.iter()
                                    .filter_map(|v| {
                                        v.get("type").and_then(Value::as_str).map(str::to_owned)
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .filter(|v| !v.is_empty())
                    {
                        OneOrManyTypes::Many(any)
                    } else {
                        return Err(ToolError::InvalidType {
                            key: name.to_owned(),
                            value: value.to_owned(),
                            need: vec!["string", "array"],
                        });
                    }
                }
            },
        };

        let default = override_cfg
            .and_then(|v| v.default.clone())
            .or_else(|| opts.get("default").cloned());

        let mut description = override_cfg.and_then(|v| v.description.clone());
        match (
            description.as_mut(),
            opts.get("description").and_then(Value::as_str),
        ) {
            (None, Some(mcp)) => {
                description = Some(mcp.to_string());
            }
            // TODO: should use `minijinja` instead.
            (Some(desc), Some(mcp)) => *desc = desc.replace("{{description}}", mcp.as_ref()),
            (Some(_) | None, None) => {}
        }

        let mut enumeration: Vec<Value> = override_cfg
            .map(|v| v.enumeration.clone())
            .into_iter()
            .flatten()
            .collect();

        if enumeration.is_empty() {
            enumeration = opts
                .get("enum")
                .and_then(|v| v.as_array())
                .into_iter()
                .flatten()
                .cloned()
                .collect();
        }

        // An MCP tool's parameter `requiredness` can be switched from `false`
        // to `true`, but not the other way around. This is because allowing
        // this could break the contract with the external tool's expectations.
        let required = required_properties.iter().any(|p| p == name);
        let required = match (required, override_cfg.map(|v| v.required)) {
            (v, None) => v,
            (true, _) => true,
            (false, Some(cfg)) => cfg,
        };

        params.insert(name.to_owned(), ToolParameterConfig {
            kind,
            default,
            description,
            required,
            enumeration,
            items: opts.get("items").and_then(|v| v.as_object()).and_then(|v| {
                Some(ToolParameterItemConfig {
                    kind: match v.get("type")? {
                        Value::String(v) => OneOrManyTypes::One(v.to_owned()),
                        Value::Array(v) => OneOrManyTypes::Many(
                            v.iter()
                                .filter_map(Value::as_str)
                                .map(str::to_owned)
                                .collect(),
                        ),
                        _ => return None,
                    },
                    default: None,
                    description: None,
                    enumeration: vec![],
                })
            }),
        });
    }

    Ok(ToolDefinition {
        name: name.to_owned(),
        description,
        parameters: params,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_tool_arguments() {
        struct TestCase {
            arguments: Map<String, Value>,
            parameters: IndexMap<String, bool>,
            want: Result<(), ToolError>,
        }

        let cases = vec![
            ("empty", TestCase {
                arguments: Map::new(),
                parameters: IndexMap::new(),
                want: Ok(()),
            }),
            ("correct", TestCase {
                arguments: Map::from_iter([("foo".to_owned(), json!("bar"))]),
                parameters: IndexMap::from_iter([
                    ("foo".to_owned(), true),
                    ("bar".to_owned(), false),
                ]),
                want: Ok(()),
            }),
            ("missing", TestCase {
                arguments: Map::new(),
                parameters: IndexMap::from_iter([("foo".to_owned(), true)]),
                want: Err(ToolError::Arguments {
                    missing: vec!["foo".to_owned()],
                    unknown: vec![],
                }),
            }),
            ("unknown", TestCase {
                arguments: Map::from_iter([("foo".to_owned(), json!("bar"))]),
                parameters: IndexMap::from_iter([("bar".to_owned(), false)]),
                want: Err(ToolError::Arguments {
                    missing: vec![],
                    unknown: vec!["foo".to_owned()],
                }),
            }),
            ("both", TestCase {
                arguments: Map::from_iter([("foo".to_owned(), json!("bar"))]),
                parameters: IndexMap::from_iter([("bar".to_owned(), true)]),
                want: Err(ToolError::Arguments {
                    missing: vec!["bar".to_owned()],
                    unknown: vec!["foo".to_owned()],
                }),
            }),
        ];

        for (name, test_case) in cases {
            let result = validate_tool_arguments(&test_case.arguments, &test_case.parameters);
            assert_eq!(result, test_case.want, "failed case: {name}");
        }
    }
}
