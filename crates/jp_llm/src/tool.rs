use std::{path::Path, sync::Arc};

use crossterm::style::Stylize as _;
use indexmap::IndexMap;
use jp_config::conversation::tool::{
    OneOrManyTypes, ResultMode, RunMode, ToolConfigWithDefaults, ToolParameterConfig,
    ToolParameterItemsConfig, ToolSource,
};
use jp_conversation::message::ToolCallResult;
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

    pub async fn call(
        &self,
        id: String,
        mut arguments: Value,
        answers: &IndexMap<String, Value>,
        mcp_client: &jp_mcp::Client,
        config: ToolConfigWithDefaults,
        root: &Path,
        editor: &Path,
    ) -> Result<ToolCallResult, ToolError> {
        info!(tool = %self.name, arguments = ?arguments, "Calling tool.");

        // If the tool call has answers to provide to the tool, it means the
        // tool already ran once, and we should not ask for confirmation again.
        let cancel_reasoning = if answers.is_empty() {
            self.prepare_run(
                config.run(),
                &mut arguments,
                config.source(),
                mcp_client,
                editor,
            )
            .await?
        } else {
            None
        };

        let result = if let Some(content) = cancel_reasoning {
            ToolCallResult {
                id,
                error: false,
                content,
            }
        } else {
            match config.source() {
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
            }
        };

        trace!(result = ?result, "Tool call completed.");
        self.prepare_result(result, config.result(), editor)
    }

    fn call_local(
        &self,
        id: String,
        arguments: &Value,
        answers: &IndexMap<String, Value>,
        config: &ToolConfigWithDefaults,
        tool: Option<&str>,
        root: &Path,
    ) -> Result<ToolCallResult, ToolError> {
        let name = tool.unwrap_or(&self.name);

        // TODO: Should we enforce at a type-level this for all tool calls, even
        // MCP?
        if let Some(args) = arguments.as_object() {
            validate_tool_arguments(
                args,
                &config
                    .parameters()
                    .iter()
                    .map(|(k, v)| (k.to_owned(), v.required))
                    .collect(),
            )?;
        }

        let command = {
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

            let tmpl = Arc::new(Environment::new());

            let program = tmpl.render_str(&command.program, &ctx).map_err(|error| {
                ToolError::TemplateError {
                    data: command.program.clone(),
                    error,
                }
            })?;

            let args = command
                .args
                .iter()
                .map(|s| tmpl.render_str(s, &ctx))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| ToolError::TemplateError {
                    data: command.args.join(" ").clone(),
                    error,
                })?;

            let expression = if command.shell {
                let cmd = std::iter::once(program.clone())
                    .chain(command.args.iter().cloned())
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
                    Ok(Outcome::Success { content }) => content,
                    Ok(Outcome::NeedsInput { question }) => {
                        return Err(ToolError::NeedsInput { question });
                    }
                };

                if output.status.success() {
                    Ok(ToolCallResult {
                        id,
                        error: false,
                        content,
                    })
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    Ok(ToolCallResult {
                        id,
                        error: true,
                        content: json!({
                            "message": format!("Tool '{name}' execution failed."),
                            "stderr": stderr,
                            "stdout": content,
                        })
                        .to_string(),
                    })
                }
            }
            Err(error) => Ok(ToolCallResult {
                id,
                error: true,
                content: json!({
                    "message": format!(
                        "Failed to execute command '{command:?}': {error}",
                    ),
                })
                .to_string(),
            }),
        }
    }

    async fn call_mcp(
        &self,
        id: String,
        arguments: &Value,
        mcp_client: &jp_mcp::Client,
        server: Option<&str>,
        tool: Option<&str>,
    ) -> Result<ToolCallResult, ToolError> {
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

        Ok(ToolCallResult {
            id,
            error: result.is_error.unwrap_or_default(),
            content,
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
        mut run_mode: RunMode,
        arguments: &mut Value,
        source: &ToolSource,
        mcp_client: &jp_mcp::Client,
        editor: &Path,
    ) -> Result<Option<String>, ToolError> {
        loop {
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
                        InlineOption::new('e', "Run tool, but first edit arguments"),
                        InlineOption::new('r', "Skip running tool, and tell assistant why"),
                    ],
                )
                .with_default('y')
                .prompt()
                .unwrap_or('n')
                {
                    'y' => return Ok(None),
                    'n' => return Ok(Some("Tool execution skipped by user.".to_string())),
                    'e' => run_mode = RunMode::Edit,
                    'r' => run_mode = RunMode::Never,
                    _ => unreachable!(),
                },
                RunMode::Always => return Ok(None),
                RunMode::Never => return Ok(Some("Tool execution skipped by user.".to_string())),
                RunMode::Edit => {}
            }

            match run_mode {
                // Never, with reasoning
                RunMode::Never => {
                    return Ok(Some(format!(
                        "Tool execution skipped by user with reasoning:\n\n{}",
                        open_editor::EditorCallBuilder::new()
                            .with_editor(open_editor::Editor::from_bin_path(editor.to_path_buf()))
                            .edit_string("_Provide reasoning for skipping tool execution_")
                            .map_err(|error| ToolError::OpenEditorError {
                                arguments: arguments.clone(),
                                error,
                            })?
                    )));
                }
                RunMode::Edit => {
                    let mut args = serde_json::to_string_pretty(&arguments).map_err(|error| {
                        ToolError::SerializeArgumentsError {
                            arguments: arguments.clone(),
                            error,
                        }
                    })?;

                    *arguments = {
                        open_editor::EditorCallBuilder::new()
                            .with_editor(open_editor::Editor::from_bin_path(editor.to_path_buf()))
                            .edit_string_mut(&mut args)
                            .map_err(|error| ToolError::OpenEditorError {
                                arguments: arguments.clone(),
                                error,
                            })?;

                        // If the user removed all data from the arguments, we
                        // consider the edit a no-op, and ask the user if they
                        // want to run the tool.
                        if args.trim().is_empty() {
                            run_mode = RunMode::Ask;
                            continue;
                        }

                        match serde_json::from_str::<Value>(&args) {
                            Ok(value) => value,

                            // If we can't parse the arguments as valid JSON, we
                            // consider the input invalid, and ask the user if
                            // they want to re-open the editor.
                            Err(error) => {
                                println!("JSON parsing error: {error}");

                                let retry = InlineSelect::new("Re-open editor?", vec![
                                    InlineOption::new('y', "Open editor to edit arguments"),
                                    InlineOption::new('n', "Skip editing, failing with error"),
                                ])
                                .with_default('y')
                                .prompt()
                                .unwrap_or('n');

                                if retry == 'n' {
                                    return Err(ToolError::EditArgumentsError {
                                        arguments: arguments.clone(),
                                        error,
                                    });
                                }

                                continue;
                            }
                        }
                    };

                    return Ok(None);
                }
                _ => unreachable!(),
            }
        }
    }

    fn prepare_result(
        &self,
        mut result: ToolCallResult,
        result_mode: ResultMode,
        editor: &Path,
    ) -> Result<ToolCallResult, ToolError> {
        loop {
            let result_mode = match result_mode {
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
                .prompt()
                .unwrap_or('n')
                {
                    'y' => return Ok(result),
                    'n' => ResultMode::Never,
                    'e' => ResultMode::Edit,
                    _ => unreachable!(),
                },
                ResultMode::Always => return Ok(result),
                mode => mode,
            };

            match result_mode {
                ResultMode::Never => {
                    return Ok(ToolCallResult {
                        id: result.id,
                        content: "Tool call result omitted by user.".into(),
                        error: false,
                    });
                }
                ResultMode::Edit => {
                    let content = open_editor::EditorCallBuilder::new()
                        .with_editor(open_editor::Editor::from_bin_path(editor.to_path_buf()))
                        .edit_string(&result.content)
                        .map_err(|error| ToolError::OpenEditorError {
                            arguments: Value::Null,
                            error,
                        })?;

                    // If the user removed all data from the result, we consider
                    // the edit a no-op, and ask the user if they want to
                    // deliver the tool results.
                    if content.trim().is_empty() {
                        continue;
                    }

                    result.content = content;
                    return Ok(result);
                }
                _ => unreachable!(),
            }
        }
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
            .or(opts.get("default").cloned());

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
                Some(ToolParameterItemsConfig {
                    kind: v.get("type")?.as_str()?.to_owned(),
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
