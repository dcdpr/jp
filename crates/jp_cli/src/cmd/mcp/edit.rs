use std::fs;

use duct::Expression;
use jp_mcp::config::McpServer;
use jp_storage::value::deep_merge;
use serde_json::Value;

use crate::{
    ctx::Ctx,
    editor::{self, Editor},
    error::Error,
    Output,
};

#[derive(Debug, clap::Args)]
pub(crate) struct Edit {
    /// Name for the MCP server
    name: String,

    /// Edit a local MCP server configuration
    #[arg(short = 'l', long = "local")]
    local: bool,

    /// How to edit the MCP server configuration.
    #[arg(short, long)]
    edit: Option<Option<Editor>>,
}

impl Edit {
    pub(crate) fn run(self, ctx: &mut Ctx) -> Output {
        let Some(cmd) = Editor::from_cli_or_config(self.edit.clone(), ctx.config.editor.clone())
            .and_then(|e| e.command())
        else {
            return Err(Error::Editor(
                "No editor configured. Use `--edit` to set the editor to use.".to_owned(),
            ))?;
        };

        if self.local {
            self.edit_local_file(ctx, cmd)
        } else {
            self.edit_workspace_file(ctx, cmd)
        }
    }

    fn edit_workspace_file(&self, ctx: &mut Ctx, cmd: Expression) -> Output {
        let workspace_file = ctx
            .workspace
            .mcp_servers_path()
            .ok_or("Workspace storage not enabled")?
            .join(format!("{}.json", self.name));

        let options = editor::Options::new(cmd)
            .with_content(serde_json::to_string_pretty(&McpServer::example())?);

        let (content, mut guard) = editor::open(workspace_file.clone(), options)?;

        serde_json::from_str::<McpServer>(&content)
            .map_err(|err| format!("Failed to parse MCP server configuration: {err}"))?;

        guard.disarm();

        Ok(format!(
            r#"Configured "{}" MCP server at: {}"#,
            self.name,
            workspace_file.display()
        )
        .into())
    }

    fn edit_local_file(&self, ctx: &mut Ctx, cmd: Expression) -> Output {
        let workspace_file = ctx
            .workspace
            .mcp_servers_path()
            .ok_or("Workspace storage not enabled")?
            .join(format!("{}.json", self.name));

        if !workspace_file.is_file() {
            return Err(
                "Local MCP server configurations must have a corresponding file in the workspace \
                 storage. Run without `--local` first."
                    .into(),
            );
        }

        let local_file = ctx
            .workspace
            .mcp_servers_local_path()
            .ok_or("Local workspace storage not configured")
            .map(|p| p.join(format!("{}.json", self.name)))?;

        let workspace_value: Value = serde_json::from_reader(fs::File::open(&workspace_file)?)?;
        let options =
            editor::Options::new(cmd).with_content(serde_json::to_string_pretty(&workspace_value)?);
        let (content, mut guard) = editor::open(local_file, options)?;
        let new_value: Value = serde_json::from_str(&content)?;

        deep_merge::<McpServer>(workspace_value, new_value)
            .map_err(|err| format!("Failed to parse MCP server configuration: {err}"))?;

        guard.disarm();

        Ok(format!(
            r#"Configured "{}" MCP server at: {}"#,
            self.name,
            workspace_file.display()
        )
        .into())
    }
}
