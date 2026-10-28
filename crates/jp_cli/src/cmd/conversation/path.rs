use camino::Utf8PathBuf;
use clap::{Arg, ArgAction, ArgMatches, Command};
use jp_conversation::ConversationId;
use jp_storage::backend::FsStorageBackend;
use jp_workspace::ConversationHandle;

use crate::{
    cli::CommandArgs,
    cmd::{ConversationLoadRequest, Output, conversation_id::PositionalIds},
    ctx::Ctx,
};

/// `jp conversation path`.
#[derive(Debug)]
pub(crate) struct Path {
    target: PositionalIds<true, true>,

    /// Print the path to `events.json`.
    events: bool,

    /// Print the path to `metadata.json`.
    metadata: bool,

    /// Print the path to `base_config.json`.
    base_config: bool,
}

impl CommandArgs for Path {
    fn augment(cmd: Command) -> Command {
        let flag = |id: &'static str, long: &'static str, help: &'static str| {
            Arg::new(id)
                .long(long)
                .action(ArgAction::SetTrue)
                .help(help)
        };

        PositionalIds::<true, true>::augment(cmd)
            .arg(flag("events", "events", "Print the path to `events.json`"))
            .arg(flag(
                "metadata",
                "metadata",
                "Print the path to `metadata.json`",
            ))
            .arg(flag(
                "base_config",
                "base-config",
                "Print the path to `base_config.json`",
            ))
    }

    fn from_matches(matches: &ArgMatches) -> Result<Self, clap::Error> {
        Ok(Self {
            target: PositionalIds::from_matches(matches)?,
            events: matches.get_flag("events"),
            metadata: matches.get_flag("metadata"),
            base_config: matches.get_flag("base_config"),
        })
    }
}

impl Path {
    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        ConversationLoadRequest::explicit_or_session(&self.target)
    }

    pub(crate) fn run(self, ctx: &mut Ctx, handles: Vec<ConversationHandle>) -> Output {
        let fs = ctx
            .fs_backend
            .as_deref()
            .ok_or("no filesystem storage configured")?;

        for handle in handles {
            let id = handle.id();
            let paths = resolve_paths(fs, &id, self.events, self.metadata, self.base_config)?;

            for path in paths {
                ctx.printer.println(path.as_str());
            }
        }

        Ok(())
    }
}

/// Resolve the requested paths for a conversation.
///
/// When no file flags are set, returns the directory path.
/// Otherwise returns the path to each requested file.
pub(crate) fn resolve_paths(
    fs: &FsStorageBackend,
    id: &ConversationId,
    events: bool,
    metadata: bool,
    base_config: bool,
) -> Result<Vec<Utf8PathBuf>, crate::cmd::Error> {
    let not_found = || format!("Conversation directory not found for {id}");

    if !events && !metadata && !base_config {
        let dir = fs.find_conversation_dir(id).ok_or_else(not_found)?;
        return Ok(vec![dir]);
    }

    let mut paths = Vec::new();

    if events {
        paths.push(fs.conversation_events_path(id).ok_or_else(not_found)?);
    }

    if metadata {
        paths.push(fs.conversation_metadata_path(id).ok_or_else(not_found)?);
    }

    if base_config {
        paths.push(fs.conversation_base_config_path(id).ok_or_else(not_found)?);
    }

    Ok(paths)
}

#[cfg(test)]
#[path = "path_tests.rs"]
mod tests;
