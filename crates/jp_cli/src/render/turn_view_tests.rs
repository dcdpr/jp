use std::{
    fmt::Write as _,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use jp_config::{
    AppConfig,
    style::reasoning::{ReasoningDisplayConfig, TruncateChars},
    types::color::Color,
};
use jp_conversation::event::ChatResponse;
use jp_printer::{OutputFormat, Printer};
use serde_json::json;

use super::*;

/// Build a `TurnView` configured with `display` for reasoning, wired to a fresh
/// separator flag that starts already owed (as if a tool result preceded it).
fn view_owing_separator(display: ReasoningDisplayConfig) -> (TurnView, Arc<AtomicBool>) {
    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    let mut style = AppConfig::new_test().style;
    style.reasoning.display = display;
    let mut view = TurnView::new(Arc::new(printer), style, None, None, RenderFlow::Replay);
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
fn whitespace_only_reasoning_preserves_tool_separator_debt() {
    // Interleaved thinking emits whitespace-only chunks between tool calls.
    // `Full` would render them, but there is nothing to render, so they supply
    // no spacing and the debt owed by the preceding result must survive.
    let (mut view, flag) = view_owing_separator(ReasoningDisplayConfig::Full);
    view.render_chat_response(&ChatResponse::reasoning("\n\n"));
    assert!(
        flag.load(Ordering::Relaxed),
        "a reasoning chunk that renders nothing must not clear the debt"
    );
}

#[test]
fn textless_static_reasoning_preserves_tool_separator_debt() {
    // A redacted thinking block arrives as an empty reasoning chunk. `Static`
    // prints its `reasoning...` line only for a chunk it renders, and a
    // textless one is not rendered, so the blank line owed before the next
    // tool header must survive.
    let (mut view, flag) = view_owing_separator(ReasoningDisplayConfig::Static);
    view.render_chat_response(&ChatResponse::reasoning(""));
    assert!(
        flag.load(Ordering::Relaxed),
        "a textless reasoning chunk renders nothing and must not clear the debt"
    );
}

#[test]
fn reasoning_past_the_truncation_budget_preserves_tool_separator_debt() {
    // Once the budget is spent, `Truncate` renders nothing for every later
    // chunk, so those chunks supply no spacing either.
    let (mut view, flag) = view_owing_separator(ReasoningDisplayConfig::Truncate(TruncateChars {
        characters: 5,
    }));
    view.render_chat_response(&ChatResponse::reasoning("12345"));
    flag.store(true, Ordering::Relaxed);

    view.render_chat_response(&ChatResponse::reasoning("67890"));
    assert!(
        flag.load(Ordering::Relaxed),
        "reasoning past the truncation budget renders nothing and must not clear the debt"
    );
}

#[test]
fn whitespace_that_fills_the_truncation_budget_clears_tool_separator_debt() {
    // `Truncate` appends its `...` elision marker whenever the taken text fills
    // the remaining budget, whitespace included, so this chunk does put
    // something on screen and does supply the spacing. Preserving the debt here
    // would pay it out alongside the chat separator the ellipsis raises, and
    // the next header would get two blank lines.
    let (mut view, flag) = view_owing_separator(ReasoningDisplayConfig::Truncate(TruncateChars {
        characters: 2,
    }));
    view.render_chat_response(&ChatResponse::reasoning("\n\n"));
    assert!(
        !flag.load(Ordering::Relaxed),
        "the rendered ellipsis supplies spacing and must clear the debt"
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

/// A prompt taken over a structured answer carries no reasoning background.
///
/// The chat renderer draws none of the JSON, so left to itself it goes on
/// reporting the region it last rendered and the interrupt menu comes up shaded
/// over content that is not.
/// A reasoning model answering `--schema` is the ordinary way to reach this:
/// every provider emits its reasoning parts and then its structured parts.
#[test]
fn a_structured_response_closes_the_region_for_later_prompts() {
    let mut style = AppConfig::new_test().style;
    style.reasoning.display = ReasoningDisplayConfig::Full;
    style.reasoning.background = Some(Color::Ansi256(236));

    let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
    let printer = Arc::new(printer);
    let mut view = TurnView::new(printer.clone(), style, None, None, RenderFlow::Live);

    view.render_chat_response_chunk(&ChatResponse::reasoning("Picking a title.\n\n"));
    printer.flush();
    out.lock().clear();

    {
        let mut prompt = printer.prompt_writer();
        write!(prompt, "Interrupted").unwrap();
    }
    printer.flush();
    assert_eq!(
        *out.lock(),
        "\x1b[48;5;236mInterrupted\x1b[49m",
        "a prompt inside the reasoning region is a visual row like any other"
    );

    view.render_chat_response_chunk(&ChatResponse::structured(json!({"title": "Jean-Pierre"})));
    printer.flush();
    out.lock().clear();

    {
        let mut prompt = printer.prompt_writer();
        write!(prompt, "Interrupted").unwrap();
    }
    printer.flush();
    assert_eq!(*out.lock(), "Interrupted");
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
    let mut view = TurnView::new(Arc::new(printer), style, None, None, RenderFlow::Replay);

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
