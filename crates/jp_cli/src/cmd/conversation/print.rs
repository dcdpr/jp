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
    #[arg(long, num_args = 0..=1, default_missing_value = "1", conflicts_with = "turn")]
    last: Option<usize>,

    /// Print a specific turn by number (1-based). Stable across new turns.
    #[arg(long, conflicts_with = "last")]
    turn: Option<usize>,

    /// Use the current workspace config instead of the per-turn config.
    ///
    /// By default, each turn is rendered with the config that was active when
    /// it was created. This flag overrides that and uses the current workspace
    /// config for all turns.
    #[arg(long, default_value_t = false)]
    current_config: bool,

    /// Print the compacted view (what the LLM sees) instead of the full
    /// history.
    #[arg(long)]
    compacted: bool,
}

impl Print {
    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        ConversationLoadRequest::explicit_or_session(&self.target)
    }

    pub(crate) fn run(self, ctx: &mut Ctx, handles: &[ConversationHandle]) -> Output {
        let selection = match self.turn {
            Some(n) => TurnSelection::Index(n),
            None => match self.last {
                Some(n) => TurnSelection::Last(n),
                None => TurnSelection::All,
            },
        };

        for handle in handles {
            Self::print_conversation(ctx, handle, &selection, self.current_config, self.compacted)?;
        }
        ctx.printer.println("");
        ctx.printer.flush();
        Ok(())
    }

    fn print_conversation(
        ctx: &mut Ctx,
        handle: &ConversationHandle,
        selection: &TurnSelection,
        current_config: bool,
        compacted: bool,
    ) -> Output {
        let mut events = ctx.workspace.events(handle)?.clone();

        if compacted {
            events.apply_projection();
        }
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

        let mut turns = events.iter_turns();
        let count = turns.len();

        match selection {
            TurnSelection::All => {
                for turn in turns {
                    renderer.render_turn(&turn);
                }
            }
            TurnSelection::Last(n) => {
                let skip = count.saturating_sub(*n);
                for turn in turns.skip(skip) {
                    renderer.render_turn(&turn);
                }
            }
            TurnSelection::Index(n) => {
                if *n == 0 || *n > count {
                    return Err(
                        format!("turn {n} out of range (conversation has {count} turns)").into(),
                    );
                }
                if let Some(turn) = turns.nth(n - 1) {
                    renderer.render_turn(&turn);
                }
            }
        }

        renderer.flush();
        Ok(())
    }
}

/// How to select which turns to print.
enum TurnSelection {
    /// Print all turns.
    All,
    /// Print the last N turns.
    Last(usize),
    /// Print a specific turn by 1-based index.
    Index(usize),
}

#[cfg(test)]
#[path = "print_tests.rs"]
mod tests;
