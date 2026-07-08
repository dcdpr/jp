use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use jp_config::{AppConfig, style::reasoning::ReasoningDisplayConfig, types::color::Color};
use jp_conversation::event::ChatResponse;
use jp_printer::{OutputFormat, Printer};

use super::*;

/// Build a `TurnView` configured with `display` for reasoning, wired to a fresh
/// separator flag that starts already owed (as if a tool result preceded it).
fn view_owing_separator(display: ReasoningDisplayConfig) -> (TurnView, Arc<AtomicBool>) {
    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    let mut style = AppConfig::new_test().style;
    style.reasoning.display = display;
    let mut view = TurnView::new(Arc::new(printer), style, None, None);
    let flag = Arc::new(AtomicBool::new(true));
    view.set_tool_separator(Arc::clone(&flag));
    (view, flag)
}

#[tokio::test]
async fn timer_reasoning_preserves_tool_separator_debt() {
    // `tool result -> timer reasoning -> next tool`: the timer line is
    // ephemeral and erased on completion, so it supplies no spacing. The blank
    // line owed before the next tool header must survive the reasoning chunk.
    let (mut view, flag) = view_owing_separator(ReasoningDisplayConfig::Timer);
    view.render_chat_response(&ChatResponse::reasoning("thinking"));
    assert!(
        flag.load(Ordering::Relaxed),
        "timer reasoning leaves no persistent output and must not clear the debt"
    );
}

#[test]
fn hidden_reasoning_preserves_tool_separator_debt() {
    let (mut view, flag) = view_owing_separator(ReasoningDisplayConfig::Hidden);
    view.render_chat_response(&ChatResponse::reasoning("thinking"));
    assert!(
        flag.load(Ordering::Relaxed),
        "hidden reasoning renders nothing and must not clear the debt"
    );
}

#[test]
fn progress_reasoning_preserves_tool_separator_debt() {
    // `tool result -> progress reasoning -> next tool`: Progress writes
    // `reasoning...` and dots with no trailing newline, so it can't separate
    // the next tool header. The owed blank line must survive the chunk; the
    // lazily emitted separator then terminates the dots line rather than the
    // header gluing onto it as `reasoning...Calling tool ...`.
    let (mut view, flag) = view_owing_separator(ReasoningDisplayConfig::Progress);
    view.render_chat_response(&ChatResponse::reasoning("thinking"));
    assert!(
        flag.load(Ordering::Relaxed),
        "progress reasoning supplies no separation and must not clear the debt"
    );
}

#[test]
fn visible_reasoning_clears_tool_separator_debt() {
    // `Full` reasoning renders persistent text, which supplies its own spacing,
    // so the owed separator is dropped to avoid a double blank line.
    let (mut view, flag) = view_owing_separator(ReasoningDisplayConfig::Full);
    view.render_chat_response(&ChatResponse::reasoning("thinking"));
    assert!(
        !flag.load(Ordering::Relaxed),
        "visible reasoning supplies spacing and clears the debt"
    );
}

#[test]
fn message_clears_tool_separator_debt() {
    let (mut view, flag) = view_owing_separator(ReasoningDisplayConfig::Hidden);
    view.render_chat_response(&ChatResponse::message("hello"));
    assert!(
        !flag.load(Ordering::Relaxed),
        "a message supplies spacing and clears the debt"
    );
}

#[test]
fn invisible_tool_call_is_transparent_to_the_reasoning_region() {
    // An invisible tool (hidden, or `show = false`, or JSON) shows no chrome,
    // so its boundary is transparent: it returns no background and leaves the
    // reasoning region intact for the next visible tool call to continue.
    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    let mut style = AppConfig::new_test().style;
    style.reasoning.display = ReasoningDisplayConfig::Full;
    style.reasoning.background = Some(Color::Ansi256(236));
    let mut view = TurnView::new(Arc::new(printer), style, None, None);

    view.render_chat_response(&ChatResponse::reasoning("Thinking\n\n"));

    assert!(
        view.enter_tool_call_region(false).is_none(),
        "an invisible tool call yields no region background"
    );
    assert!(
        view.enter_tool_call_region(true).is_some(),
        "the region survives the invisible tool call, so the next visible tool call still \
         continues it"
    );
}
