use jp_config::PartialAppConfig;
use jp_workspace::{ConversationHandle, Workspace};

use super::{ConversationLoadRequest, Output};
use crate::ctx::{Ctx, IntoPartialAppConfig};

mod archive;
pub(crate) mod compact;
mod edit;
pub(crate) mod fork;
mod grep;
mod ls;
mod path;
mod print;
mod rm;
mod show;
mod unarchive;
pub(crate) mod summarize;
mod use_;

#[derive(Debug, clap::Args)]
pub(crate) struct Conversation {
    #[command(subcommand)]
    command: Commands,
}

impl Conversation {
    pub(crate) async fn run(self, ctx: &mut Ctx, handles: Vec<ConversationHandle>) -> Output {
        match self.command {
            Commands::Show(args) => args.run(ctx, handles),
            Commands::Remove(args) => args.run(ctx, handles).await,
            Commands::Edit(args) => args.run(ctx, handles).await,
            Commands::Fork(args) => args.run(ctx, &handles).await,
            Commands::Compact(args) => args.run(ctx, handles).await,
            Commands::Grep(args) => args.run(ctx, handles),
            Commands::Print(args) => args.run(ctx, &handles),
            Commands::Path(args) => args.run(ctx, handles),
            Commands::List(args) => args.run(ctx, &handles),
            Commands::Use(args) => args.run(ctx, handles),
            Commands::Archive(args) => args.run(ctx, handles).await,
            Commands::Unarchive(args) => args.run(ctx),
        }
    }

    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        match &self.command {
            Commands::Show(args) => args.conversation_load_request(),
            Commands::Remove(args) => args.conversation_load_request(),
            Commands::Edit(args) => args.conversation_load_request(),
            Commands::Fork(args) => args.conversation_load_request(),
            Commands::Compact(args) => args.conversation_load_request(),
            Commands::Grep(args) => args.conversation_load_request(),
            Commands::Print(args) => args.conversation_load_request(),
            Commands::Path(args) => args.conversation_load_request(),
            Commands::List(args) => args.conversation_load_request(),
            Commands::Use(args) => args.conversation_load_request(),
            Commands::Archive(args) => args.conversation_load_request(),
            Commands::Unarchive(args) => args.conversation_load_request(),
        }
    }
}

#[derive(Debug, clap::Subcommand)]
enum Commands {
    /// Remove conversations.
    #[command(name = "rm", aliases = ["remove", "rem", "delete", "del"])]
    Remove(rm::Rm),

    /// List conversations.
    #[command(name = "ls", alias = "list", visible_alias = "l")]
    List(ls::Ls),

    /// Show conversation details.
    #[command(name = "show", visible_alias = "s")]
    Show(show::Show),

    /// Set the active conversation.
    #[command(name = "use", visible_alias = "u")]
    Use(use_::Use),

    /// Edit conversation details.
    #[command(name = "edit", visible_alias = "e")]
    Edit(edit::Edit),

    /// Fork a conversation.
    #[command(name = "fork", visible_alias = "f")]
    Fork(fork::Fork),

    /// Compact a conversation to reduce context size.
    ///
    /// Appends a compaction overlay that instructs the LLM projection layer
    /// to strip reasoning blocks and/or tool call content from the specified
    /// range. The original events are preserved.
    #[command(name = "compact")]
    Compact(compact::Compact),

    /// Search through conversation history.
    #[command(name = "grep", alias = "rg", visible_alias = "g")]
    Grep(grep::Grep),

    /// Print conversation history to the terminal.
    #[command(name = "print", visible_alias = "p")]
    Print(print::Print),

    /// Print the filesystem path to a conversation.
    #[command(name = "path")]
    Path(path::Path),

    /// Archive conversations.
    #[command(name = "archive", visible_alias = "a")]
    Archive(archive::Archive),

    /// Unarchive conversations.
    #[command(name = "unarchive", visible_alias = "ua")]
    Unarchive(unarchive::Unarchive),
}

impl IntoPartialAppConfig for Conversation {
    fn apply_cli_config(
        &self,
        workspace: Option<&Workspace>,
        partial: PartialAppConfig,
        merged_config: Option<&PartialAppConfig>,
        handles: &[jp_workspace::ConversationHandle],
    ) -> Result<PartialAppConfig, Box<dyn std::error::Error + Send + Sync>> {
        match &self.command {
            Commands::Compact(args) => {
                args.apply_cli_config(workspace, partial, merged_config, handles)
            }
            Commands::Fork(args) => {
                args.apply_cli_config(workspace, partial, merged_config, handles)
            }
            _ => Ok(partial),
        }
    }
}
