use jp_config::{
    conversation::tool::style::{
        self, DisplayStyleConfig, ErrorStyleConfig, InlineResults, ParametersStyle,
    },
    style::{reasoning::ReasoningDisplayConfig, typewriter::DelayDuration},
};
use jp_conversation::compaction::resolve_range;
use jp_llm::tool::InvocationContext;
use jp_workspace::ConversationHandle;

use crate::{
    cmd::{
        ConversationLoadRequest, Output,
        conversation_id::PositionalIds,
        turn_range::{Bound, TurnRange},
    },
    ctx::Ctx,
    render::{ConfigSource, TurnRenderer},
};

/// Brief-mode tool display style: no arguments, no results, no file links.
const BRIEF_TOOL_STYLE: DisplayStyleConfig = DisplayStyleConfig {
    hidden: false,
    parameters: ParametersStyle::Off,
    inline_results: InlineResults::Off,
    results_file_link: style::LinkStyle::Off,
    error: ErrorStyleConfig {
        inline_results: None,
        results_file_link: None,
    },
};

/// Chat-mode tool display style: tool calls are fully hidden.
const CHAT_TOOL_STYLE: DisplayStyleConfig = DisplayStyleConfig {
    hidden: true,
    parameters: ParametersStyle::Off,
    inline_results: InlineResults::Off,
    results_file_link: style::LinkStyle::Off,
    error: ErrorStyleConfig {
        inline_results: None,
        results_file_link: None,
    },
};

/// Full-mode tool display style: everything visible, nothing truncated.
const FULL_TOOL_STYLE: DisplayStyleConfig = DisplayStyleConfig {
    hidden: false,
    parameters: ParametersStyle::Json,
    inline_results: InlineResults::Full,
    results_file_link: style::LinkStyle::Full,
    error: ErrorStyleConfig {
        inline_results: None,
        results_file_link: None,
    },
};

#[derive(Debug, clap::Args)]
pub(crate) struct Print {
    #[command(flatten)]
    target: PositionalIds<true, true>,

    /// Which turns to print.
    ///
    /// Without any selector, prints the whole conversation.
    #[command(flatten)]
    range: TurnRange,

    /// Use the current workspace config instead of the per-turn config.
    ///
    /// By default, each turn is rendered with the config that was active when
    /// it was created.
    /// This flag overrides that and uses the current workspace config for all
    /// turns.
    #[arg(long, default_value_t = false)]
    current_config: bool,

    /// Output style preset.
    ///
    /// - `user`: Show only user messages.
    ///   Hides assistant messages, reasoning, and tool calls entirely.
    /// - `chat`: Show only user and assistant messages.
    ///   Hides reasoning and tool calls entirely.
    /// - `brief`: Hide reasoning, tool arguments, and tool results.
    ///   Shows only user messages, assistant messages, and tool call headers.
    /// - `full`: Show everything including reasoning, tool arguments, and
    ///   untruncated tool results.
    #[arg(long, short = 's', value_enum)]
    style: Option<PrintStyle>,

    /// Print the compacted view (what the LLM sees) instead of the full
    /// history.
    #[arg(long)]
    compacted: bool,
}

/// Output style presets for `jp conversation print`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum PrintStyle {
    /// Show only user messages; hide assistant messages, reasoning, and tool
    /// calls entirely.
    User,
    /// Show only user and assistant messages; hide reasoning and tool calls
    /// entirely.
    Chat,
    /// Hide reasoning, tool arguments, and tool results.
    Brief,
    /// Show everything: full reasoning, tool arguments, and untruncated tool
    /// results.
    Full,
}

impl Print {
    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        ConversationLoadRequest::explicit_or_session(&self.target)
    }

    pub(crate) fn run(self, ctx: &mut Ctx, handles: &[ConversationHandle]) -> Output {
        for handle in handles {
            Self::print_conversation(
                ctx,
                handle,
                &self.range,
                self.current_config,
                self.style,
                self.compacted,
            )?;
        }
        ctx.printer.println("");
        ctx.printer.flush();
        Ok(())
    }

    fn print_conversation(
        ctx: &mut Ctx,
        handle: &ConversationHandle,
        range: &TurnRange,
        current_config: bool,
        print_style: Option<PrintStyle>,
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

        let user_only = matches!(print_style, Some(PrintStyle::User));

        let assistant_name = cfg.assistant.name.clone();
        let model_id = Some(cfg.assistant.model.id.resolved().to_string());

        let invocation = InvocationContext {
            workspace_id: ctx.workspace.id().to_string(),
            conversation_id: handle.id().to_string(),
        };

        let mut renderer = TurnRenderer::new(
            ctx.printer.clone(),
            render_style,
            tools_config,
            assistant_name,
            model_id,
            root,
            ctx.term.is_tty,
            source,
            invocation,
        );
        renderer.set_user_only(user_only);

        let count = events.turn_count();

        // `--last 0` explicitly selects nothing.
        if range.is_empty() {
            renderer.flush();
            return Ok(());
        }

        // `--turn` names specific turns; an out-of-range endpoint is an error.
        if let Some(n) = range.turn_out_of_range(count) {
            return Err(format!("turn {n} out of range (conversation has {count} turns)").into());
        }

        let from = match range.resolve_from(&events) {
            Bound::Empty => {
                renderer.flush();
                return Ok(());
            }
            Bound::Default => None,
            Bound::At(b) => Some(b),
        };
        let to = match range.resolve_to(&events) {
            Bound::Empty => {
                renderer.flush();
                return Ok(());
            }
            Bound::Default => None,
            Bound::At(b) => Some(b),
        };

        // A `from > to` or otherwise empty range selects nothing.
        let Some(selected) = resolve_range(&events, from, to) else {
            renderer.flush();
            return Ok(());
        };

        for turn in events.iter_turns() {
            if (selected.from_turn..=selected.to_turn).contains(&turn.index()) {
                renderer.render_turn(&turn);
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
        PrintStyle::User | PrintStyle::Chat => (ReasoningDisplayConfig::Hidden, CHAT_TOOL_STYLE),
        PrintStyle::Brief => (ReasoningDisplayConfig::Hidden, BRIEF_TOOL_STYLE),
        PrintStyle::Full => (ReasoningDisplayConfig::Full, FULL_TOOL_STYLE),
    };

    style.reasoning.display = reasoning_display;
    tools_config.defaults.style = tool_style.clone();
    for (_name, tool) in tools_config.iter_mut() {
        tool.style = Some(tool_style.clone());
    }
}

#[cfg(test)]
#[path = "print_tests.rs"]
mod tests;
