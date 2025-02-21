use anyhow::Result;
use clap::{Args, ValueEnum};
use exodus_trace::span;

use crate::{context::Context, openrouter::Client, process_question};

#[derive(Args)]
pub struct AskArgs {
    #[arg(required = true)]
    question: String,

    /// Configure reasoning step.
    ///
    /// By default the reasoning step is performed and hidden from the user.
    /// This option allows you to change that default.
    #[arg(short, long)]
    pub reasoning: Reasoning,
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum Reasoning {
    /// Disable reasoning step.
    ///
    /// This differs from `Hide` in that the reasoning model will not be
    /// invoked, allowing the chat model to generate a response without any
    /// reasoning.
    Disable,

    /// Show the reasoning output to the user.
    Show,

    /// Run the reasoning step, but only show a progress indicator.
    ///
    /// Sometimes reasoning can take a long time, this option gives you a visual
    /// indication that the reasoning step is running.
    Progress,

    /// Run the reasoning step, but hide the output from the user.
    #[default]
    Hide,
}

pub async fn run(ctx: Context, args: &AskArgs) -> Result<()> {
    let _g = span!();

    let client = Client::from_config(&ctx.config)?;
    process_question(&client, &ctx, &args.question, args.reasoning).await
}
