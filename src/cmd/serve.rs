use anyhow::Result;
use clap::Args;
use exodus_trace::{info, span};

use crate::{context::Context, start_server};

#[derive(Debug, Args)]
pub struct ServeArgs {
    /// Port to listen on (overrides config file)
    #[arg(short, long)]
    port: Option<u16>,
}

pub async fn run(mut ctx: Context, args: &ServeArgs) -> Result<()> {
    let _g = span!();

    if let Some(port) = args.port {
        ctx.config.server.port = port;
    }

    info!("Starting server on port {}", ctx.config.server.port);
    start_server(ctx.into()).await
}
