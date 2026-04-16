use jp_config::style::typewriter::DelayDuration;
use jp_workspace::ConversationHandle;

use crate::{
    cmd::{ConversationLoadRequest, Output, conversation_id::PositionalIds},
    ctx::Ctx,
    render::{ConfigSource, TurnRenderer},
};

#[derive(Debug, clap::Args)]
pub(crate) struct Print {
    #[command(flatten)]
    target: PositionalIds<true, true>,

    /// Print only the last N turns. Without a value, prints the last turn.
    #[arg(long, num_args = 0..=1, default_missing_value = "1")]
    last: Option<usize>,

    /// Use the current workspace config instead of the per-turn config.
    ///
    /// By default, each turn is rendered with the config that was active when
    /// it was created. This flag overrides that and uses the current workspace
    /// config for all turns.
    #[arg(long, default_value_t = false)]
    current_config: bool,
}

impl Print {
    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        ConversationLoadRequest::explicit_or_session(&self.target)
    }

    pub(crate) fn run(self, ctx: &mut Ctx, handles: &[ConversationHandle]) -> Output {
        for handle in handles {
            Self::print_conversation(ctx, handle, self.last, self.current_config)?;
        }
        ctx.printer.println("");
        ctx.printer.flush();
        Ok(())
    }

    fn print_conversation(
        ctx: &mut Ctx,
        handle: &ConversationHandle,
        last: Option<usize>,
        current_config: bool,
    ) -> Output {
        let events = ctx.workspace.events(handle)?.clone();
        let cfg = ctx.config();

        let root = ctx
            .storage_path()
            .unwrap_or(ctx.workspace.root())
            .to_path_buf();

        let source = if current_config {
            ConfigSource::Fixed
        } else {
            ConfigSource::PerTurn
        };

        // Disable typewriter delays — print replays content instantly.
        let mut style = cfg.style.clone();
        style.typewriter.text_delay = DelayDuration::instant();
        style.typewriter.code_delay = DelayDuration::instant();

        let mut renderer = TurnRenderer::new(
            ctx.printer.clone(),
            style,
            cfg.conversation.tools.clone(),
            root,
            ctx.term.is_tty,
            source,
        );

        let turns = events.iter_turns();
        let skip = last.map_or(0, |n| turns.len().saturating_sub(n));

        for turn in turns.skip(skip) {
            renderer.render_turn(&turn);
        }

        renderer.flush();
        Ok(())
    }
}

#[cfg(test)]
#[path = "print_tests.rs"]
mod tests;
