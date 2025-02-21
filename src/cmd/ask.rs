use core::fmt;

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
    #[arg(short, long, default_value_t)]
    pub reasoning: Reasoning,

    /// Override the system prompt for the chat model.
    ///
    /// This takes precedence over the config file setting.
    #[arg(long)]
    pub chat_prompt: Option<String>,

    /// Override the system prompt for the reasoning model.
    ///
    /// This takes precedence over the config file setting.
    #[arg(long)]
    pub reasoning_prompt: Option<String>,
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

impl fmt::Display for Reasoning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disable => write!(f, "disable"),
            Self::Show => write!(f, "show"),
            Self::Progress => write!(f, "progress"),
            Self::Hide => write!(f, "hide"),
        }
    }
}

pub async fn run(mut ctx: Context, args: &AskArgs) -> Result<()> {
    let _g = span!();

    let client = Client::from_config(&ctx.config)?;

    if let Some(chat_prompt) = &args.chat_prompt {
        *ctx.config.llm.chat.system_prompt_mut() = chat_prompt.clone();
    }

    if let Some(reasoning_prompt) = &args.reasoning_prompt {
        *ctx.config.llm.reasoning.system_prompt_mut() = reasoning_prompt.clone();
    }

    process_question(&client, &ctx, &args.question, args.reasoning).await
}
