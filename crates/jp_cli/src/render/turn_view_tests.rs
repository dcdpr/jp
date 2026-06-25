use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use jp_config::{AppConfig, style::reasoning::ReasoningDisplayConfig};
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
