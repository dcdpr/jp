use comfy_table::{Cell, Row};

use crate::{cmd::Success, ctx::Ctx, Output};

#[derive(Debug, clap::Args)]
pub(crate) struct List {}

impl List {
    #[expect(clippy::unused_self, clippy::unnecessary_wraps)]
    pub(crate) fn run(self, ctx: &mut Ctx) -> Output {
        let servers: Vec<_> = ctx.workspace.mcp_servers().collect();

        if servers.is_empty() {
            return Ok("No MCP servers configured.".into());
        }

        let header = Row::from(vec![
            "Name",
            "Command",
            "Args",
            "Environment Variables",
            "Active",
        ]);
        let mut rows = vec![];

        for server in servers {
            let mut row = Row::new();
            row.add_cell(Cell::new(server.id.to_string()));
            match server.transport {
                jp_mcp::transport::Transport::Stdio(ref config) => {
                    row.add_cell(Cell::new(config.command.display()));
                    row.add_cell(Cell::new(config.args.join(" ")));
                    row.add_cell(Cell::new(config.environment_variables.join(" ")));
                }
            }

            row.add_cell(Cell::new(
                ctx.config.conversation.mcp_servers.contains(&server.id),
            ));

            rows.push(row);
        }

        Ok(Success::Table { header, rows })
    }
}
