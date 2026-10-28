use camino::Utf8Path;
use clap::{Arg, ArgAction, ArgGroup, ArgMatches, Command};
use jp_config::fs::{ConfigFile, ConfigLoader, ConfigLoaderError, user_global_config_dir};
use jp_workspace::ConversationHandle;

use super::{ConversationLoadRequest, Output};
use crate::{
    cli::{CommandArgs, subcommand_of, unread_subcommand},
    ctx::Ctx,
};

mod fmt;
mod set;
mod show;

/// `jp config`.
#[derive(Debug)]
pub(crate) struct Config {
    command: Commands,
}

impl CommandArgs for Config {
    fn augment(cmd: Command) -> Command {
        cmd.subcommand_required(true)
            .arg_required_else_help(true)
            .subcommand(
                show::Show::augment(Command::new("show")).about("Show the current configuration"),
            )
            .subcommand(set::Set::augment(Command::new("set")).about("Set a configuration option"))
            .subcommand(fmt::Fmt::augment(Command::new("fmt")).about("Format a configuration file"))
    }

    fn from_matches(matches: &ArgMatches) -> Result<Self, clap::Error> {
        let command = match subcommand_of(matches)? {
            ("show", sub) => Commands::Show(show::Show::from_matches(sub)?),
            ("set", sub) => Commands::Set(set::Set::from_matches(sub)?),
            ("fmt", sub) => Commands::Fmt(fmt::Fmt::from_matches(sub)?),
            (name, _) => return Err(unread_subcommand(name)),
        };

        Ok(Self { command })
    }
}

impl Config {
    pub(crate) async fn run(self, ctx: &mut Ctx, handles: Vec<ConversationHandle>) -> Output {
        match self.command {
            Commands::Show(args) => {
                debug_assert!(handles.is_empty());
                args.run(ctx)
            }
            Commands::Set(args) => args.run(ctx, handles).await,
            Commands::Fmt(args) => {
                debug_assert!(handles.is_empty());
                args.run(ctx)
            }
        }
    }

    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        match &self.command {
            Commands::Set(args) => args.conversation_load_request(),
            Commands::Show(_) | Commands::Fmt(_) => ConversationLoadRequest::none(),
        }
    }
}

#[derive(Debug)]
enum Commands {
    Show(show::Show),
    Set(set::Set),
    Fmt(fmt::Fmt),
}

/// Which configuration file a command acts on; at most one is given.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Target {
    /// The workspace's user-specific configuration file.
    pub(crate) user_workspace: bool,

    /// The global user-specific configuration file.
    pub(crate) user_global: bool,

    /// The `.jp.toml` nearest the working directory, searched upwards.
    pub(crate) cwd: bool,
}

impl Target {
    /// Declare the target flags on `cmd` as the group `group`, each described
    /// by its `help` and conflicting with `conflicts`.
    fn augment(
        cmd: Command,
        group: &'static str,
        help: [&'static str; 3],
        conflicts: Option<&'static str>,
    ) -> Command {
        let [user_workspace, user_global, cwd] = help;
        let flag = |id: &'static str, long: &'static str, help: &'static str| {
            let arg = Arg::new(id)
                .long(long)
                .action(ArgAction::SetTrue)
                .help(help);
            match conflicts {
                Some(other) => arg.conflicts_with(other),
                None => arg,
            }
        };

        cmd.group(ArgGroup::new(group).multiple(false))
            .arg(flag("user_workspace", "user-workspace", user_workspace).group(group))
            .arg(flag("user_global", "user-global", user_global).group(group))
            .arg(flag("cwd", "cwd", cwd).group(group))
    }

    fn from_matches(matches: &ArgMatches) -> Self {
        Self {
            user_workspace: matches.get_flag("user_workspace"),
            user_global: matches.get_flag("user_global"),
            cwd: matches.get_flag("cwd"),
        }
    }

    /// Get the the configuration file, if one exists.
    pub(crate) fn config_file(self, ctx: &Ctx) -> Result<Option<ConfigFile>, ConfigLoaderError> {
        let mut loader = ConfigLoader {
            file_stem: "config".into(),
            ..Default::default()
        };

        if self.user_workspace {
            ctx.user_storage_path().map(|p| loader.load(p)).transpose()
        } else if self.user_global {
            user_global_config_dir(
                std::env::home_dir()
                    .as_deref()
                    .and_then(|p| Utf8Path::from_path(p)),
            )
            .map(|p| loader.load(p))
            .transpose()
        } else if self.cwd {
            loader.file_stem = ".jp".into();
            loader.recurse_up = true;
            loader.recurse_stop_at = Some(ctx.workspace.root().to_path_buf());

            // The same directory the `.jp.toml` chain is *read* from, so a
            // from-anywhere run writes the file it would load, and the
            // recurse-up walk stays anchored at the workspace root above it.
            loader.load(ctx.exec.config_cwd()).map(Some)
        } else {
            ctx.storage_path().map(|p| loader.load(p)).transpose()
        }
    }
}
