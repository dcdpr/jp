use std::path::PathBuf;

use jp_mcp::config::{McpServer, McpServerId};

use crate::{ctx::Ctx, Output};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Name for the MCP server
    pub name: String,

    /// Command to execute
    pub command: String,

    /// Environment variables to expose (in format NAME=VALUE)
    #[arg(short = 'e', long = "env", value_delimiter = ',')]
    pub environment_variables: Vec<String>,
}

impl Args {
    #[expect(clippy::unnecessary_wraps)]
    pub fn run(self, ctx: &mut Ctx) -> Output {
        let (command, args) = self.command.split_once(' ').unwrap_or((&self.command, ""));

        let transport = jp_mcp::transport::Stdio {
            command: PathBuf::from(command),
            args: args.split_whitespace().map(ToString::to_string).collect(),
            environment_variables: self.environment_variables,
        };

        let config = McpServer {
            id: McpServerId::new(&self.name),
            transport: transport.into(),
        };

        ctx.workspace.create_mcp_server(config);

        Ok(format!("Added MCP server: {}", self.name).into())
    }
}
