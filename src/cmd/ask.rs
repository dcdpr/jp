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

    #[arg(short = 's', long)]
    pub web_search: Option<WebSearch>,

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum WebSearch {
    /// Disable web search.
    Disable,

    /// Enable web search for both LLMs.
    Enable,

    /// Enable web search only for "reasoning" LLM.
    Reasoning,

    /// Enable web search only for "chat" LLM.
    Chat,
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
        ctx.config.llm.chat.system_prompt = chat_prompt.to_owned();
    }

    if let Some(reasoning_prompt) = &args.reasoning_prompt {
        ctx.config.llm.reasoning.system_prompt = reasoning_prompt.to_owned();
    }

    match args.web_search {
        Some(WebSearch::Enable) => {
            ctx.config.llm.chat.web_search = true;
            ctx.config.llm.reasoning.web_search = true;
        }
        Some(WebSearch::Reasoning) => {
            ctx.config.llm.chat.web_search = false;
            ctx.config.llm.reasoning.web_search = true;
        }
        Some(WebSearch::Chat) => {
            ctx.config.llm.chat.web_search = true;
            ctx.config.llm.reasoning.web_search = false;
        }
        Some(WebSearch::Disable) => {
            ctx.config.llm.chat.web_search = false;
            ctx.config.llm.reasoning.web_search = false;
        }
        None => {}
    }

    process_question(&client, &ctx, &args.question, args.reasoning).await
}
