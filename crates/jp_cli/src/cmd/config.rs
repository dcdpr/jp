use camino::{FromPathBufError, Utf8Path, Utf8PathBuf};
use jp_config::fs::{ConfigFile, ConfigLoader, ConfigLoaderError, user_global_config_path};
use jp_workspace::ConversationHandle;

use super::{ConversationLoadRequest, Output};
use crate::ctx::Ctx;

mod fmt;
mod set;
mod show;

#[derive(Debug, clap::Args)]
pub(crate) struct Config {
    #[command(subcommand)]
    command: Commands,
}

impl Config {
    pub(crate) fn run(self, ctx: &mut Ctx, handles: Vec<ConversationHandle>) -> Output {
        match self.command {
            Commands::Show(args) => {
                debug_assert!(handles.is_empty());
                args.run(ctx)
            }
            Commands::Set(args) => args.run(ctx, handles),
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

#[derive(Debug, clap::Subcommand)]
enum Commands {
    /// Show the current configuration.
    #[command(name = "show")]
    Show(show::Show),

    /// Set a configuration option.
    #[command(name = "set")]
    Set(set::Set),

    /// Format a configuration file.
    #[command(name = "fmt")]
    Fmt(fmt::Fmt),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, clap::Args)]
#[group(required = false, multiple = false)]
struct Target {
    /// The workspace's user-specific configuration target.
    #[arg(long)]
    pub(crate) user_workspace: bool,

    /// The global user-specific configuration target.
    #[arg(long)]
    pub(crate) user_global: bool,

    /// The current-working-directory configuration target, recursively upwards.
    #[arg(long)]
    pub(crate) cwd: bool,
}

impl Target {
    /// Get the the configuration file, if one exists.
    pub(crate) fn config_file(self, ctx: &Ctx) -> Result<Option<ConfigFile>, ConfigLoaderError> {
        let mut loader = ConfigLoader {
            file_stem: "config".into(),
            ..Default::default()
        };

        if self.user_workspace {
            ctx.workspace
                .user_storage_path()
                .map(|p| loader.load(p))
                .transpose()
        } else if self.user_global {
            user_global_config_path(
                std::env::home_dir()
                    .as_deref()
                    .and_then(|p| Utf8Path::from_path(p)),
            )
            .map(|mut p| {
                if p.is_file()
                    && let Some(stem) = p.file_name()
                    && let Some(path) = p.parent()
                {
                    loader.file_stem = stem.to_owned().into();
                    p = path.to_path_buf();
                }

                loader.load(p)
            })
            .transpose()
        } else if self.cwd {
            loader.file_stem = ".jp".into();
            loader.recurse_up = true;
            loader.recurse_stop_at = Some(ctx.workspace.root().to_path_buf());

            let current_dir = Utf8PathBuf::try_from(std::env::current_dir()?)
                .map_err(FromPathBufError::into_io_error)?;

            loader.load(current_dir).map(Some)
        } else {
            ctx.workspace
                .storage_path()
                .map(|p| loader.load(p))
                .transpose()
        }
    }
}
