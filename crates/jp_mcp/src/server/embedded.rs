use std::{any::type_name, collections::HashMap, path::PathBuf, sync::Arc};

use minijinja::Environment;
use rmcp::{model, Error};
use serde_json::{from_str, json, Map, Value};
use tokio::{process::Command, sync::Mutex};

use crate::tool::{McpTool, McpToolId};

#[derive(Clone, Debug)]
pub struct EmbeddedServer {
    tools: Arc<Mutex<HashMap<McpToolId, McpTool>>>,
    root: PathBuf,
    tmpl: Arc<Environment<'static>>,
}

impl EmbeddedServer {
    #[must_use]
    pub fn new(tools: HashMap<McpToolId, McpTool>, root: PathBuf) -> Self {
        Self {
            tools: Arc::new(Mutex::new(tools)),
            root,
            tmpl: Arc::new(Environment::new()),
        }
    }

    pub async fn get_command_path(&self, id: &McpToolId) -> Result<PathBuf, Error> {
        self.tools
            .lock()
            .await
            .get(id)
            .cloned()
            .and_then(|t| t.command.first().cloned().map(Into::into))
            .ok_or_else(|| Error::new(model::ErrorCode::METHOD_NOT_FOUND, id.to_string(), None))
    }

    fn build_command(
        &self,
        id: &McpToolId,
        mut tool: McpTool,
        arguments: Option<Map<String, Value>>,
    ) -> Result<Command, Error> {
        let ctx = json!({
            "tool": {
                "name": id.to_string(),
                "arguments": arguments.unwrap_or_default(),
            },
            "workspace": {
                "path": self.root.to_string_lossy().into_owned(),
            },
        });

        if tool.command.is_empty() {
            return Err(Error::internal_error(
                format!("Tool is missing command: {id}"),
                None,
            ));
        }

        let cmd = tool.command.remove(0);
        let args = tool
            .command
            .into_iter()
            .map(|s| self.tmpl.render_str(&s, &ctx))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| Error::internal_error(format!("Template rendering error: {e}"), None))?;

        let mut command = Command::new(&cmd);
        command.args(args);
        command.current_dir(&self.root);

        Ok(command)
    }

    pub async fn list_all_tools(&self) -> Result<Vec<model::Tool>, Error> {
        let tools_map = self.tools.lock().await;
        let mut tools = Vec::new();

        for (id, tool) in tools_map.iter() {
            let mut properties = Map::new();
            let mut required_properties = Vec::new();

            for prop in &tool.properties {
                let mut schema = Map::new();
                let name = get_property("name", id, prop, Value::as_str)?;
                if prop
                    .get("required")
                    .is_some_and(|v| v.as_bool().unwrap_or(false))
                {
                    required_properties.push(id.to_string());
                }

                for (key, value) in prop {
                    if key == "name" || key == "type" || key == "required" {
                        continue;
                    }

                    schema.insert(key.into(), value.clone());
                }

                properties.insert(name.to_string(), schema.into());
            }

            let mut input_schema = Map::from_iter([
                ("type".to_owned(), "object".into()),
                ("additionalProperties".to_owned(), false.into()),
                ("properties".to_owned(), properties.into()),
            ]);

            if !required_properties.is_empty() {
                input_schema.insert("required".to_owned(), Value::Null);
            }

            tools.push(model::Tool {
                name: id.to_string().into(),
                description: Some(tool.description.clone().into()),
                input_schema: Arc::new(input_schema),
                annotations: None,
            });
        }

        Ok(tools)
    }

    pub async fn run_tool(
        &self,
        request: model::CallToolRequestParam,
    ) -> Result<model::CallToolResult, Error> {
        let model::CallToolRequestParam { name, arguments } = request;
        let id = McpToolId::new(name.to_string());

        let tool =
            self.tools.lock().await.get(&id).cloned().ok_or_else(|| {
                Error::new(model::ErrorCode::METHOD_NOT_FOUND, id.to_string(), None)
            })?;

        let mut command = self.build_command(&id, tool, arguments)?;

        match command.output().await {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if output.status.success() {
                    Ok(model::CallToolResult {
                        is_error: Some(false),
                        content: vec![match from_str::<Value>(&stdout) {
                            Ok(value) => model::Content::json(value)?,
                            Err(_) => model::Content::text(stdout),
                        }],
                    })
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    Ok(model::CallToolResult {
                        is_error: Some(true),
                        content: vec![model::Content::json(json!({
                            "message": format!("Tool '{id}' execution failed."),
                            "stderr": stderr,
                            "stdout": stdout,
                        }))?],
                    })
                }
            }
            Err(error) => Ok(model::CallToolResult {
                is_error: Some(true),
                content: vec![model::Content::json(json!({
                    "message": format!(
                        "Failed to execute command '{}': {error}",
                        command.as_std().get_program().to_string_lossy(),
                    ),
                }))?],
            }),
        }
    }
}

fn get_property<'a, T: Into<Value>>(
    kind: &'a str,
    id: &'a McpToolId,
    prop: &'a Map<String, Value>,
    f: impl Fn(&'a Value) -> Option<T>,
) -> Result<T, Error> {
    prop.get(kind)
        .ok_or_else(|| {
            Error::invalid_params(format!("tool `{id}` property missing field: {kind}"), None)
        })
        .and_then(|v| {
            f(v).ok_or_else(|| {
                Error::invalid_params(
                    format!(
                        "tool `{id}` property `{kind}` must be type: {}",
                        type_name::<T>()
                    ),
                    None,
                )
            })
        })
}
