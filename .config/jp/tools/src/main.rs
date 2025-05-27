use mcp_attr::{server::serve_stdio, Result};
use tools::ToolsServer;

#[tokio::main]
#[expect(clippy::result_large_err)]
async fn main() -> Result<()> {
    serve_stdio(ToolsServer::default()).await?;
    Ok(())
}
