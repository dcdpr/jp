use clap::{ArgMatches, Command};
use jp_config::PartialAppConfig;
use jp_workspace::{ConversationHandle, Workspace};

use super::{ConversationLoadRequest, Output};
use crate::{
    cli::{CommandArgs, subcommand_of, unread_subcommand},
    ctx::{Ctx, IntoPartialAppConfig},
};

mod archive;
pub(crate) mod compact;
mod edit;
pub(crate) mod fork;
mod grep;
mod label;
mod ls;
mod path;
mod print;
mod rm;
mod show;
pub(crate) mod summarize;
mod title;
mod unarchive;
mod use_;

/// `jp conversation`.
#[derive(Debug)]
pub(crate) struct Conversation {
    command: Commands,
}

impl CommandArgs for Conversation {
    fn augment(cmd: Command) -> Command {
        cmd.subcommand_required(true)
            .arg_required_else_help(true)
            .subcommand(
                rm::Rm::augment(Command::new("rm"))
                    .about("Remove conversations")
                    .aliases(["remove", "rem", "delete", "del"]),
            )
            .subcommand(
                ls::Ls::augment(Command::new("ls"))
                    .about("List conversations")
                    .alias("list"),
            )
            .subcommand(
                show::Show::augment(Command::new("show"))
                    .about("Show conversation details")
                    .visible_alias("s"),
            )
            .subcommand(
                use_::Use::augment(Command::new("use"))
                    .about("Set the active conversation")
                    .visible_alias("u"),
            )
            .subcommand(
                edit::Edit::augment(Command::new("edit"))
                    .about("Edit conversation details")
                    .visible_alias("e"),
            )
            .subcommand(
                fork::Fork::augment(Command::new("fork"))
                    .about("Fork a conversation")
                    .visible_alias("f"),
            )
            .subcommand(
                compact::Compact::augment(Command::new("compact"))
                    .about("Compact a conversation to reduce context size")
                    .long_about(
                        "Compact a conversation to reduce context size.\n\nAppends a compaction \
                         overlay that instructs the LLM projection layer to strip reasoning \
                         blocks and/or tool call content from the specified range. The original \
                         events are preserved.",
                    ),
            )
            .subcommand(
                title::Title::augment(Command::new("title"))
                    .about("Generate a conversation title with an LLM")
                    .long_about(
                        "Generate a conversation title with an LLM.\n\nOffers the generated \
                         candidates as a picker, and applies the chosen one as the conversation's \
                         title. Use `edit --title \"TITLE\"` to set a title without involving a \
                         model.",
                    )
                    .visible_alias("t"),
            )
            .subcommand(
                grep::Grep::augment(Command::new("grep"))
                    .about("Search through conversation history")
                    .visible_alias("g")
                    .alias("rg"),
            )
            .subcommand(
                label::Label::augment(Command::new("label"))
                    .about("Manage the labels on a conversation")
                    .long_about(
                        "Manage the labels on a conversation.\n\n`jp c label add`, `set`, `rm`, \
                         and `ls`; a bare `jp c label` lists.",
                    )
                    .visible_alias("l")
                    .alias("labels"),
            )
            .subcommand(
                print::Print::augment(Command::new("print"))
                    .about("Print conversation history to the terminal")
                    .visible_alias("p"),
            )
            .subcommand(
                path::Path::augment(Command::new("path"))
                    .about("Print the filesystem path to a conversation"),
            )
            .subcommand(
                archive::Archive::augment(Command::new("archive"))
                    .about("Archive conversations")
                    .visible_alias("a"),
            )
            .subcommand(
                unarchive::Unarchive::augment(Command::new("unarchive"))
                    .about("Unarchive conversations")
                    .visible_alias("ua"),
            )
    }

    fn from_matches(matches: &ArgMatches) -> Result<Self, clap::Error> {
        let command = match subcommand_of(matches)? {
            ("rm", sub) => Commands::Remove(rm::Rm::from_matches(sub)?),
            ("ls", sub) => Commands::List(ls::Ls::from_matches(sub)?),
            ("show", sub) => Commands::Show(show::Show::from_matches(sub)?),
            ("use", sub) => Commands::Use(use_::Use::from_matches(sub)?),
            ("edit", sub) => Commands::Edit(edit::Edit::from_matches(sub)?),
            ("fork", sub) => Commands::Fork(fork::Fork::from_matches(sub)?),
            ("compact", sub) => Commands::Compact(compact::Compact::from_matches(sub)?),
            ("title", sub) => Commands::Title(title::Title::from_matches(sub)?),
            ("grep", sub) => Commands::Grep(grep::Grep::from_matches(sub)?),
            ("label", sub) => Commands::Label(label::Label::from_matches(sub)?),
            ("print", sub) => Commands::Print(print::Print::from_matches(sub)?),
            ("path", sub) => Commands::Path(path::Path::from_matches(sub)?),
            ("archive", sub) => Commands::Archive(archive::Archive::from_matches(sub)?),
            ("unarchive", sub) => Commands::Unarchive(unarchive::Unarchive::from_matches(sub)?),
            (name, _) => return Err(unread_subcommand(name)),
        };

        Ok(Self { command })
    }
}

impl Conversation {
    pub(crate) async fn run(self, ctx: &mut Ctx, handles: Vec<ConversationHandle>) -> Output {
        match self.command {
            Commands::Show(args) => args.run(ctx, handles),
            Commands::Remove(args) => args.run(ctx, handles).await,
            Commands::Edit(args) => args.run(ctx, handles).await,
            Commands::Fork(args) => args.run(ctx, &handles).await,
            Commands::Compact(args) => args.run(ctx, handles).await,
            Commands::Title(args) => args.run(ctx, handles).await,
            Commands::Grep(args) => args.run(ctx, handles),
            Commands::Label(args) => args.run(ctx, handles).await,
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
            Commands::Title(args) => args.conversation_load_request(),
            Commands::Grep(args) => args.conversation_load_request(),
            Commands::Label(args) => args.conversation_load_request(),
            Commands::Print(args) => args.conversation_load_request(),
            Commands::Path(args) => args.conversation_load_request(),
            Commands::List(args) => args.conversation_load_request(),
            Commands::Use(args) => args.conversation_load_request(),
            Commands::Archive(args) => args.conversation_load_request(),
            Commands::Unarchive(args) => args.conversation_load_request(),
        }
    }
}

impl IntoPartialAppConfig for Conversation {
    fn apply_cli_config(
        &self,
        workspace: Option<&Workspace>,
        partial: PartialAppConfig,
        merged_config: Option<&PartialAppConfig>,
    ) -> std::result::Result<PartialAppConfig, Box<dyn std::error::Error + Send + Sync>> {
        match &self.command {
            Commands::Compact(args) => args.apply_cli_config(workspace, partial, merged_config),
            Commands::Title(args) => args.apply_cli_config(workspace, partial, merged_config),
            Commands::Show(_)
            | Commands::Remove(_)
            | Commands::Edit(_)
            | Commands::Fork(_)
            | Commands::Grep(_)
            | Commands::Label(_)
            | Commands::Print(_)
            | Commands::Path(_)
            | Commands::List(_)
            | Commands::Use(_)
            | Commands::Archive(_)
            | Commands::Unarchive(_) => Ok(partial),
        }
    }
}

#[derive(Debug)]
enum Commands {
    Remove(rm::Rm),
    List(ls::Ls),
    Show(show::Show),
    Use(use_::Use),
    Edit(edit::Edit),
    Fork(fork::Fork),
    Compact(compact::Compact),
    Title(title::Title),
    Grep(grep::Grep),
    Label(label::Label),
    Print(print::Print),
    Path(path::Path),
    Archive(archive::Archive),
    Unarchive(unarchive::Unarchive),
}
