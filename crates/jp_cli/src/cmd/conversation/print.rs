use jp_config::{
    conversation::tool::style::{self, DisplayStyleConfig, InlineResults, ParametersStyle},
    style::{reasoning::ReasoningDisplayConfig, typewriter::DelayDuration},
};
use jp_workspace::ConversationHandle;

use crate::{
    cmd::{ConversationLoadRequest, Output, conversation_id::PositionalIds},
    ctx::Ctx,
    render::{ConfigSource, TurnRenderer},
};

/// Brief-mode tool display style: no arguments, no results, no file links.
const BRIEF_TOOL_STYLE: DisplayStyleConfig = DisplayStyleConfig {
    hidden: false,
    parameters: ParametersStyle::Off,
    inline_results: InlineResults::Off,
    results_file_link: style::LinkStyle::Off,
};

/// Full-mode tool display style: everything visible, nothing truncated.
const FULL_TOOL_STYLE: DisplayStyleConfig = DisplayStyleConfig {
    hidden: false,
    parameters: ParametersStyle::Json,
    inline_results: InlineResults::Full,
    results_file_link: style::LinkStyle::Full,
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

    /// Output style preset.
    ///
    /// - `brief`: Hide reasoning, tool arguments, and tool results. Shows
    ///   only user messages, assistant messages, and tool call headers.
    /// - `full`: Show everything including reasoning, tool arguments, and
    ///   untruncated tool results.
    #[arg(long, short = 's', value_enum)]
    style: Option<PrintStyle>,
}

/// Output style presets for `jp conversation print`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum PrintStyle {
    /// Hide reasoning, tool arguments, and tool results.
    Brief,
    /// Show everything: full reasoning, tool arguments, and untruncated
    /// tool results.
    Full,
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
            Self::print_conversation(ctx, handle, &selection, self.current_config, self.style)?;
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
        print_style: Option<PrintStyle>,
    ) -> Output {
        let events = ctx.workspace.events(handle)?.clone();
        let cfg = ctx.config();

        let root = ctx
            .storage_path()
            .unwrap_or(ctx.workspace.root())
            .to_path_buf();

        let source = if current_config || print_style.is_some() {
            ConfigSource::Fixed
        } else {
            ConfigSource::PerTurn
        };

        // Disable typewriter delays — print replays content instantly.
        let mut render_style = cfg.style.clone();
        render_style.typewriter.text_delay = DelayDuration::instant();
        render_style.typewriter.code_delay = DelayDuration::instant();

        let mut tools_config = cfg.conversation.tools.clone();

        if let Some(preset) = print_style {
            apply_style_preset(preset, &mut render_style, &mut tools_config);
        }

        let mut renderer = TurnRenderer::new(
            ctx.printer.clone(),
            render_style,
            tools_config,
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

/// Apply a style preset to the rendering config and tool config.
fn apply_style_preset(
    preset: PrintStyle,
    style: &mut jp_config::style::StyleConfig,
    tools_config: &mut jp_config::conversation::tool::ToolsConfig,
) {
    let (reasoning_display, tool_style) = match preset {
        PrintStyle::Brief => (ReasoningDisplayConfig::Hidden, BRIEF_TOOL_STYLE),
        PrintStyle::Full => (ReasoningDisplayConfig::Full, FULL_TOOL_STYLE),
    };

    style.reasoning.display = reasoning_display;
    tools_config.defaults.style = tool_style.clone();
    for (_name, tool) in tools_config.iter_mut() {
        tool.style = Some(tool_style.clone());
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
