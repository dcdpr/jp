use camino::Utf8PathBuf;
use jp_conversation::ConversationId;
use jp_workspace::{ConversationHandle, Workspace};

use crate::{
    cmd::{ConversationLoadRequest, Output, conversation_id::PositionalIds},
    ctx::Ctx,
};

#[derive(Debug, clap::Args)]
pub(crate) struct Path {
    #[command(flatten)]
    target: PositionalIds<true, true>,

    /// Print the path to `events.json`.
    #[arg(long)]
    events: bool,

    /// Print the path to `metadata.json`.
    #[arg(long)]
    metadata: bool,

    /// Print the path to `base_config.json`.
    #[arg(long)]
    base_config: bool,
}

impl Path {
    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        ConversationLoadRequest::explicit_or_session(&self.target.ids)
    }

    pub(crate) fn run(self, ctx: &mut Ctx, handles: Vec<ConversationHandle>) -> Output {
        for handle in handles {
            let id = handle.id();
            let paths = resolve_paths(
                &ctx.workspace,
                &id,
                self.events,
                self.metadata,
                self.base_config,
            )?;

            for path in paths {
                ctx.printer.println(path.as_str());
            }
        }

        Ok(())
    }
}

/// Resolve the requested paths for a conversation.
///
/// When no file flags are set, returns the directory path. Otherwise returns
/// the path to each requested file.
pub(crate) fn resolve_paths(
    workspace: &Workspace,
    id: &ConversationId,
    events: bool,
    metadata: bool,
    base_config: bool,
) -> Result<Vec<Utf8PathBuf>, crate::cmd::Error> {
    let not_found = || format!("Conversation directory not found for {id}");

    if !events && !metadata && !base_config {
        let dir = workspace.conversation_dir(id).ok_or_else(not_found)?;
        return Ok(vec![dir]);
    }

    let mut paths = Vec::new();

    if events {
        paths.push(
            workspace
                .conversation_events_path(id)
                .ok_or_else(not_found)?,
        );
    }

    if metadata {
        paths.push(
            workspace
                .conversation_metadata_path(id)
                .ok_or_else(not_found)?,
        );
    }

    if base_config {
        paths.push(
            workspace
                .conversation_base_config_path(id)
                .ok_or_else(not_found)?,
        );
    }

    Ok(paths)
}

#[cfg(test)]
#[path = "path_tests.rs"]
mod tests;
