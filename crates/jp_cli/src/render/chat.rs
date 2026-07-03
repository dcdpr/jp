//! Chat rendering for both the live-stream query pipeline and conversation
//! replay.
//!
//! The [`ChatRenderer`] handles rendering of both `ChatRequest` (user messages)
//! and `ChatResponse` events (reasoning and message content) to the terminal.
//!
//! # Rendering Pipeline
//!
//! ```text
//! ChatResponse          Valid markdown blocks        Formatted output
//!     │                         │                          │
//!     ▼                         ▼                          ▼
//! ┌────────┐               ┌───────────┐            ┌──────────┐
//! │ Buffer │ ────────────▶ │ Formatter │ ─────────▶ │ Printer  │
//! └────────┘               └───────────┘            └──────────┘
//! ```
//!
//! # Display Modes
//!
//! Reasoning content can be displayed in different modes:
//!
//! | Mode          | Behavior                                 |
//! | ------------- | ---------------------------------------- |
//! | `Hidden`      | Don't render reasoning (still persisted) |
//! | `Full`        | Render all reasoning tokens              |
//! | `Truncate(N)` | Render first N characters, then "..."    |
//! | `Progress`    | Show "reasoning..." then dots            |
//! | `Static`      | Show "reasoning..." once                 |
//! | `Timer`       | Show a running timer, erase when done    |

use std::{fmt::Write as _, sync::Arc, time::Duration};

use crossterm::style::Stylize as _;
use jp_config::style::{
    StyleConfig,
    reasoning::{ReasoningDisplayConfig, TruncateChars},
};
use jp_conversation::event::ChatResponse;
use jp_md::{
    buffer::{Buffer, Event, Fixups},
    format::{
        BackgroundFill, CodeBlockState, DefaultBackground, Formatter, TerminalOptions,
        render_separator,
    },
    theme,
};
use jp_printer::{PrintableExt as _, Printer};
use tracing::warn;

use crate::timer::{LineTimer, spawn_line_timer};

/// The kind of content last pushed into the renderer.
///
/// Used to detect content-type transitions so that the markdown buffer can be
/// force-flushed before a different kind of content is rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContentKind {
    Reasoning,
    Message,
    ToolCall,
}

/// Renders chat events to the terminal.
///
/// Handles user messages, assistant reasoning, and assistant message content,
/// applying the configured display mode for reasoning.
/// Tracks content-kind transitions to insert appropriate spacing between
/// different content types (e.g. blank lines between tool calls and message
/// text).
pub struct ChatRenderer {
    buffer: Buffer,
    formatter: Formatter,
    printer: Arc<Printer>,
    config: StyleConfig,
    last_content_kind: Option<ContentKind>,
    reasoning_chars_count: usize,
    /// State for the current streaming fenced code block, if any.
    code_block: Option<CodeBlockState>,
    /// Active reasoning timer, used by `Timer` display mode.
    reasoning_timer: Option<LineTimer>,
    /// Whether a rendered reasoning block still owes its trailing inter-block
    /// separator.
    ///
    /// Reasoning blocks defer that separator so its background can be chosen
    /// once the following content is known: shaded when more reasoning follows
    /// (the gap stays inside the reasoning region), unstyled when reasoning
    /// gives way to a message, tool call, or end of stream.
    reasoning_separator_pending: bool,
    /// Post-processing fixups for LLM quirks in the event stream.
    fixups: Fixups,
    /// Accumulated source of the top-level paragraph currently streaming.
    ///
    /// Empty when no paragraph is mid-stream.
    /// The renderer re-renders this whole buffer on each
    /// [`Event::ParagraphChunk`] and prints only the newly committed delta, so
    /// streamed output is byte-identical to rendering the finished paragraph in
    /// one shot.
    para_source: String,
    /// Bytes of `para_source`'s render already printed (the stable prefix).
    para_emitted: usize,
}

impl ChatRenderer {
    pub fn new(printer: Arc<Printer>, config: StyleConfig) -> Self {
        let pretty = printer.pretty_printing_enabled();
        let formatter = formatter_from_config(&config, pretty);
        // Configure the printer's bounded-latency controller from the
        // typewriter style. `max_latency = 0` (the default) leaves the
        // controller disabled, preserving the original static per-character
        // delay behavior.
        printer.set_max_latency(config.typewriter.max_latency.into());
        Self {
            buffer: Buffer::new(),
            formatter,
            printer,
            config,
            last_content_kind: None,
            reasoning_chars_count: 0,
            code_block: None,
            reasoning_timer: None,
            reasoning_separator_pending: false,
            fixups: Fixups::llm_quirks(),
            para_source: String::new(),
            para_emitted: 0,
        }
    }

    /// Render a `ChatResponse` (assistant output).
    ///
    /// Structured responses are ignored — they are handled by the
    /// `StructuredRenderer` in the live-stream path and inline in print.
    pub fn render_response(&mut self, response: &ChatResponse) {
        match response {
            ChatResponse::Reasoning { reasoning } => self.render_reasoning(reasoning),
            ChatResponse::Message { message } => self.render_message(message),
            ChatResponse::Structured { .. } => {}
        }
    }

    /// Render a user message (`ChatRequest` content).
    ///
    /// Formats the content as a complete markdown block.
    /// Callers are responsible for emitting any preceding role header via
    /// [`Self::render_role_header`] — the renderer no longer emits a trailing
    /// separator on its own.
    pub fn render_request(&mut self, content: &str) {
        self.flush_on_transition(ContentKind::Message);
        self.flush();

        let formatted = self
            .formatter
            .format_terminal(content.trim_end())
            .unwrap_or_else(|_| content.trim_end().to_owned());
        self.printer.println(&formatted);

        self.last_content_kind = Some(ContentKind::Message);
    }

    /// Render a labeled role-boundary header.
    ///
    /// Draws a single line with the label embedded near the left and an
    /// optional dimmed suffix appended after it, with `─` characters filling
    /// the remaining width.
    /// An optional `detail` is appended dimmed at the right edge, after the
    /// fill — e.g. `── alice ──…── turn 2, 12 minutes ago ──`.
    /// Used by [`TurnRenderer`] to mark which participant is speaking next —
    /// e.g. `── alice ──…` before a user turn, `── jp
    /// (anthropic/claude-opus-4-8) ──…` before an assistant turn.
    ///
    /// Replaces the old plain `---` HR separator and disambiguates JP's turn
    /// boundaries from any HR markdown the assistant itself emits.
    ///
    /// [`TurnRenderer`]: super::TurnRenderer
    pub fn render_role_header(&mut self, label: &str, suffix: Option<&str>, detail: Option<&str>) {
        self.flush();

        let pretty = self.printer.pretty_printing_enabled();
        let line = build_role_header_line(
            label,
            suffix,
            detail,
            self.config.markdown.wrap_width,
            pretty,
        );

        self.printer.println("");
        self.printer.println(&line);
        self.printer.println("");

        self.last_content_kind = None;
    }

    /// Flush the markdown buffer if the content kind is changing.
    ///
    /// When the LLM switches from one content type to another (e.g. reasoning
    /// → message, or message → tool call), any partial markdown sitting in
    /// the buffer must be emitted immediately.
    /// Without this, content before the transition would only appear after the
    /// next block boundary — which may not arrive until much later (or never,
    /// if a tool call follows).
    ///
    /// Always cancels any active ephemeral reasoning chrome (e.g. the `Timer`
    /// display): persistent content arriving — even of the same `ContentKind`
    /// as before — must stop the running timer, since the timer line and the
    /// upcoming content share the terminal row.
    fn flush_on_transition(&mut self, next: ContentKind) {
        self.cancel_reasoning_timer();
        if let Some(prev) = self.last_content_kind
            && prev != next
        {
            self.flush();
            if prev == ContentKind::ToolCall {
                self.printer.println("");
            }
        }

        self.last_content_kind = Some(next);
    }

    fn render_reasoning(&mut self, content: &str) {
        match self.config.reasoning.display {
            // Even though reasoning is hidden, a reasoning block is a
            // semantic boundary: flush any buffered message content so that
            // surrounding message chunks render as distinct paragraphs
            // rather than being glued together by the markdown buffer.
            ReasoningDisplayConfig::Hidden => self.flush(),

            ReasoningDisplayConfig::Full => {
                self.flush_on_transition(ContentKind::Reasoning);
                self.render_content(content);
            }

            ReasoningDisplayConfig::Truncate(TruncateChars { characters }) => {
                self.flush_on_transition(ContentKind::Reasoning);

                let remaining = characters.saturating_sub(self.reasoning_chars_count);

                if remaining > 0 {
                    let mut data: String = content.chars().take(remaining).collect();
                    if data.chars().count() == remaining {
                        data.push_str("...\n\n");
                    }

                    self.render_content(&data);
                }

                self.reasoning_chars_count += content.chars().count();
            }

            ReasoningDisplayConfig::Progress => {
                if self.last_content_kind == Some(ContentKind::Reasoning) {
                    self.printer.eprint(".");
                } else {
                    self.flush_on_transition(ContentKind::Reasoning);
                    self.printer.eprint("reasoning...");
                }
            }

            ReasoningDisplayConfig::Static => {
                if self.last_content_kind != Some(ContentKind::Reasoning) {
                    self.flush_on_transition(ContentKind::Reasoning);
                    self.printer.eprintln("reasoning...\n");
                }
            }

            ReasoningDisplayConfig::Timer => {
                // Timer is ephemeral chrome on stderr: it writes a line
                // that's erased again on cancel, leaving no persistent
                // stdout output. Like `Hidden`, it must not go through
                // `flush_on_transition` — that would commit a blank-line
                // separator on stdout (when leaving a `ToolCall` block)
                // that no later content ever "earns" back. Use the timer
                // token itself for re-entry detection instead of
                // `last_content_kind`.
                if self.reasoning_timer.is_none() {
                    self.flush();

                    self.reasoning_timer = spawn_line_timer(
                        self.printer.clone(),
                        self.printer.pretty_printing_enabled(),
                        Duration::from_millis(300),
                        Duration::from_millis(100),
                        |secs, _status| {
                            format!("\r\x1b[K\x1b[2m\u{23f1} Reasoning\u{2026} {secs:.1}s\x1b[22m")
                        },
                    );
                }
            }

            ReasoningDisplayConfig::Summary => {
                // Summary mode requires an async LLM call to summarize
                // reasoning. This is not yet implemented.
                todo!("Summary mode requires async LLM summarization")
            }
        }
    }

    fn render_message(&mut self, content: &str) {
        self.flush_on_transition(ContentKind::Message);
        self.render_content(content);
    }

    fn render_content(&mut self, content: &str) {
        self.buffer.push(content);
        self.flush_buffer_blocks();
    }

    /// Flush any complete markdown blocks in the buffer.
    fn flush_buffer_blocks(&mut self) {
        while let Some(raw_event) = self.buffer.next() {
            // Apply fixups (LLM quirk corrections) to the raw event.
            if let Some(event) = self.fixups.apply(raw_event) {
                self.handle_event(event);
            }
        }
    }

    /// Render a single (post-fixup) buffer event to the printer.
    ///
    /// Shared by the steady-state streaming loop and the end-of-region flush so
    /// both paths treat fenced-code events identically: escalated fences,
    /// syntax-highlighted lines, and a trailing separator at the top level.
    fn handle_event(&mut self, event: Event) {
        match event {
            // `Event::Flush` is the terminal block of a content region (the
            // buffer's end-of-region drain); `Event::Block` is a mid-stream
            // block. Only the terminal block forces its trailing separator, so
            // a region ending in a tight list still gets a blank line before
            // whatever follows.
            Event::Block { content, indent } => self.print_block(&content, indent, false),
            Event::Flush { content, indent } => self.print_block(&content, indent, true),
            Event::FencedCodeStart {
                ref language,
                indent,
                ..
            } => {
                // A code block inside reasoning consumes the deferred
                // separator the same way another reasoning block would.
                if self.last_content_kind == Some(ContentKind::Reasoning) {
                    self.emit_pending_reasoning_separator(true);
                }
                self.code_block = Some(self.formatter.begin_code_block(language));
                let bg = self.terminal_options(0).default_background;
                let rendered = self
                    .formatter
                    .render_code_fence(&format!("{event}\n"), bg.as_ref());
                self.print_code(&rendered, indent);
            }
            Event::FencedCodeLine { content, indent } => {
                let bg = self.terminal_options(0).default_background;
                let rendered = if let Some(ref mut state) = self.code_block {
                    self.formatter
                        .render_code_line(&content, state, bg.as_ref())
                } else {
                    content
                };
                self.print_code(&rendered, indent);
            }
            Event::FencedCodeEnd { fence, indent } => {
                let bg = self.terminal_options(0).default_background;
                // At top level (indent == 0), append a blank-line
                // separator to keep the conventional gap between a
                // closing fence and the next block. Inside a list
                // item (indent > 0) the separator would break the
                // visual flow of the list, so we emit the fence on
                // its own and let the next event handle its own
                // separation.
                let mut rendered = self
                    .formatter
                    .render_code_fence(&format!("{fence}\n"), bg.as_ref());
                if indent == 0 {
                    rendered.push_str(&render_separator(bg.as_ref()));
                }
                self.print_code(&rendered, indent);
                self.code_block = None;
            }
            Event::ParagraphChunk {
                content,
                indent,
                last,
            } => self.handle_paragraph_chunk(&content, indent, last),
            // Every `Event` variant is handled above; this arm exists only
            // because `Event` is `#[non_exhaustive]`. Fail loudly if a future
            // variant reaches here rather than silently dropping its output.
            other => unreachable!("unhandled buffer event: {other:?}"),
        }
    }

    /// Print a raw code string with the code typewriter delay.
    ///
    /// The content is already highlighted and has background applied by the
    /// formatter's streaming code block API.
    /// `indent` is the visual column the renderer should put each line at (used
    /// when the code block is inside a list item).
    fn print_code(&self, content: &str, indent: usize) {
        let delay = self.config.typewriter.code_delay;
        let content = if indent == 0 {
            content.to_string()
        } else {
            indent_lines(content, indent)
        };
        self.printer.print(content.typewriter(delay.into()));
    }

    /// Format `source` with the reasoning-aware terminal options for the
    /// current content kind.
    ///
    /// Shared by `print_block` and `handle_paragraph_chunk` so the options
    /// derivation and the `format_terminal_with` call have one home; the two
    /// callers differ only in their emission strategy (a whole block vs. a
    /// stable streamed delta) and in the separator flags they pass here.
    fn format_styled(
        &self,
        source: &str,
        indent: usize,
        suppress_trailing: bool,
        force_trailing: bool,
    ) -> String {
        let mut opts = self.terminal_options(indent);
        opts.suppress_trailing_separator = suppress_trailing;
        opts.force_trailing_separator = force_trailing;
        self.formatter
            .format_terminal_with(source, &opts)
            .unwrap_or_else(|_| source.to_string())
    }

    fn print_block(&mut self, block: &str, indent: usize, terminal: bool) {
        // Skip whitespace-only blocks. These can appear when the LLM emits
        // blank text content blocks (e.g. "\n\n" between interleaved thinking
        // blocks) that survive a buffer flush.
        if block.trim().is_empty() {
            return;
        }

        let is_reasoning = self.last_content_kind == Some(ContentKind::Reasoning);

        // A reasoning block following another reasoning block: emit the
        // deferred separator shaded so the gap stays inside the reasoning
        // region.
        if is_reasoning {
            self.emit_pending_reasoning_separator(true);
        }

        // Defer a reasoning block's trailing separator (its shading depends on
        // what follows, unknown until the next event). A terminal (flushed)
        // message block ends its region, so a tight list here keeps its trailing
        // separator; reasoning never forces, since it defers via
        // `suppress_trailing_separator`.
        let formatted = self.format_styled(block, indent, is_reasoning, terminal && !is_reasoning);

        let delay = self.config.typewriter.text_delay;
        self.printer.print(formatted.typewriter(delay.into()));

        if is_reasoning {
            self.reasoning_separator_pending = true;
        }
    }

    /// Render a streamed slice of a top-level paragraph.
    ///
    /// Accumulates the paragraph's source, re-renders the whole paragraph with
    /// the same options [`print_block`] uses, and prints only the newly
    /// committed prefix delta — holding the in-progress visual line until a
    /// later chunk (or `last`) commits it.
    /// The concatenation of all deltas equals the one-shot block render, so
    /// streamed output is byte-identical to non-streaming.
    ///
    /// [`print_block`]: Self::print_block
    fn handle_paragraph_chunk(&mut self, content: &str, indent: usize, last: bool) {
        let is_reasoning = self.last_content_kind == Some(ContentKind::Reasoning);

        // First chunk of this paragraph: a reasoning paragraph consumes the
        // deferred separator shaded, exactly as `print_block` does for a Block.
        if self.para_source.is_empty() && is_reasoning {
            self.emit_pending_reasoning_separator(true);
        }

        self.para_source.push_str(content);

        // Intermediate chunks suppress the trailing separator (it sits past the
        // held in-progress line anyway); the final chunk suppresses it only for
        // reasoning, exactly as `print_block` does. A paragraph is never a tight
        // list, so `force_trailing_separator` stays false.
        let rendered = self.format_styled(&self.para_source, indent, is_reasoning || !last, false);

        // Cut at the last committed newline, holding the in-progress visual
        // line. `format_terminal_with` always finalizes with a trailing newline,
        // so the last line of a non-final render is never a real wrap commit;
        // drop it before searching. At `last` the whole render is committed.
        let cut = if last {
            rendered.len()
        } else {
            match rendered.trim_end_matches('\n').rfind('\n') {
                Some(i) => i + 1,
                None => 0,
            }
        };

        if cut > self.para_emitted {
            let delta = rendered[self.para_emitted..cut].to_string();
            let delay = self.config.typewriter.text_delay;
            self.printer.print(delta.typewriter(delay.into()));
            self.para_emitted = cut;
        }

        if last {
            if is_reasoning {
                self.reasoning_separator_pending = true;
            }
            self.para_source.clear();
            self.para_emitted = 0;
        }
    }

    /// Build per-block terminal options based on the current content kind and
    /// visual indent.
    fn terminal_options(&self, indent: usize) -> TerminalOptions {
        TerminalOptions {
            default_background: if self.last_content_kind == Some(ContentKind::Reasoning) {
                self.reasoning_background()
            } else {
                None
            },
            indent,
            suppress_trailing_separator: false,
            force_trailing_separator: false,
        }
    }

    /// The full-width background fill configured for reasoning content, if any.
    fn reasoning_background(&self) -> Option<DefaultBackground> {
        self.config
            .reasoning
            .background
            .map(|color| DefaultBackground {
                param: crate::format::color_to_bg_param(color),
                fill: BackgroundFill::Terminal,
            })
    }

    /// Emit the separator owed by the previously rendered reasoning block.
    ///
    /// Reasoning blocks render without their trailing inter-block separator so
    /// its background can be decided once the following content is known.
    /// When `shaded`, the separator carries the reasoning background (the gap
    /// sits between two reasoning blocks); otherwise it is unstyled (reasoning
    /// is giving way to other content).
    fn emit_pending_reasoning_separator(&mut self, shaded: bool) {
        if !self.reasoning_separator_pending {
            return;
        }
        self.reasoning_separator_pending = false;

        let background = if shaded {
            self.reasoning_background()
        } else {
            None
        };
        let separator = render_separator(background.as_ref());
        let delay = self.config.typewriter.text_delay;
        self.printer.print(separator.typewriter(delay.into()));
    }

    pub fn flush(&mut self) {
        self.cancel_reasoning_timer();

        // Drain the buffer's end-of-region events through the same fixup +
        // render path as streaming. A code block left open by the stream is
        // closed here with a matched, escalated fence (recognized or
        // synthesized by `flush_events`) instead of leaking its body as
        // re-parsed markdown.
        for raw_event in self.buffer.flush_events() {
            if let Some(event) = self.fixups.apply(raw_event) {
                self.handle_event(event);
            }
        }
        self.code_block = None;

        // Reaching a flush means we're leaving the current content region (a
        // content-kind transition, a role header, or end of stream). Emit any
        // deferred reasoning separator unstyled so the gap to whatever follows
        // isn't shaded.
        self.emit_pending_reasoning_separator(false);
    }

    /// Signal that the current typewriter producer is done emitting.
    ///
    /// Called by the coordinator on `Event::Finished` after the renderer has
    /// flushed its remaining content.
    /// Switches the printer's bounded-latency controller into drain mode so the
    /// per-character delay can no longer grow as the queue empties.
    pub fn signal_typewriter_drain(&self) {
        self.printer.mark_typewriter_drained();
    }

    /// Cancel the reasoning timer if one is running.
    ///
    /// Dropping the handle cancels the background task; the synchronous clear
    /// here guarantees the line is gone before the caller's next write, since
    /// this method cannot await the task's own asynchronous clear.
    fn cancel_reasoning_timer(&mut self) {
        if self.reasoning_timer.take().is_some() {
            let _ = write!(self.printer.err_writer(), "\r\x1b[K");
        }
    }

    /// Whether the configured reasoning display supplies its own separation
    /// before following content.
    ///
    /// `Static`, `Full`, and `Truncate` leave terminated visible output, so a
    /// following tool header is cleanly separated from them.
    /// `Hidden` renders nothing, `Timer` writes a stderr line it erases again
    /// on completion, and `Progress` writes `reasoning...` plus dots with no
    /// trailing newline — none of these separate the next header, so a caller
    /// coordinating inter-block spacing must keep the owed separator across
    /// them.
    pub(crate) fn reasoning_supplies_separation(&self) -> bool {
        !matches!(
            self.config.reasoning.display,
            ReasoningDisplayConfig::Hidden
                | ReasoningDisplayConfig::Timer
                | ReasoningDisplayConfig::Progress
        )
    }

    /// Transition renderer state to tool call mode.
    pub fn transition_to_tool_call(&mut self) {
        self.last_content_kind = Some(ContentKind::ToolCall);
    }

    /// Reset the renderer state, discarding any buffered content.
    ///
    /// Used when the current streaming cycle is being interrupted and a new one
    /// will start (e.g., after a Reply or Continue action).
    /// The partial content in the buffer has already been captured by the event
    /// builder, so it's safe to discard.
    pub fn reset(&mut self) {
        self.cancel_reasoning_timer();
        self.buffer = Buffer::new();
        let pretty = self.printer.pretty_printing_enabled();
        self.formatter = formatter_from_config(&self.config, pretty);
        self.last_content_kind = None;
        self.reasoning_separator_pending = false;
        self.reasoning_chars_count = 0;
        self.code_block = None;
        self.fixups = Fixups::llm_quirks();
        // Drop any held in-progress paragraph line; the buffered source was
        // captured by the event builder, so it is safe to discard.
        self.para_source.clear();
        self.para_emitted = 0;
    }
}

/// Build a labeled horizontal rule used as a role-boundary marker.
///
/// Layout: `── <label> [(<suffix>)] ──… [<detail> ──]` filling `width`
/// columns.
/// In `pretty` mode, the label is bold and the optional suffix and detail are
/// dimmed.
/// Plain mode emits the same characters without ANSI styling so it survives
/// ANSI-stripping pipes (e.g. `jp c print | grep`).
fn build_role_header_line(
    label: &str,
    suffix: Option<&str>,
    detail: Option<&str>,
    width: usize,
    pretty: bool,
) -> String {
    let suffix_part = suffix.map(|s| format!(" ({s})")).unwrap_or_default();
    let detail_part = detail.map(|d| format!(" {d} ──")).unwrap_or_default();

    // Compute fill against the unstyled width so ANSI escapes don't throw
    // off the column count.
    let left = format!("── {label}{suffix_part} ");
    let fill = width
        .saturating_sub(left.chars().count() + detail_part.chars().count())
        .max(3);
    let dashes = "─".repeat(fill);

    if pretty {
        let label_styled = label.bold();
        let suffix_styled = if suffix.is_some() {
            format!(" {}", suffix_part.trim_start().dim())
        } else {
            String::new()
        };
        let detail_styled = detail
            .map(|d| format!(" {} ──", d.dim()))
            .unwrap_or_default();
        format!("── {label_styled}{suffix_styled} {dashes}{detail_styled}")
    } else {
        format!("{left}{dashes}{detail_part}")
    }
}

/// Prepend `indent` spaces to every line of `content`.
///
/// Used to indent streaming code-block lines that originate from inside a list
/// item.
/// ANSI escape sequences are zero-width and don't count as visible content: the
/// indent prefix is emitted before the first *visible* character on a new line,
/// not before any leading escapes.
/// This matters because syntax highlighters routinely close a line with
/// `\n\x1b[0m` (reset *after* the newline); without this rule, the trailing
/// reset would be misread as the start of a new line and trigger an extra
/// prefix, pushing the next line's visible content `indent` columns too far
/// right.
fn indent_lines(content: &str, indent: usize) -> String {
    let prefix = " ".repeat(indent);
    let mut out = String::with_capacity(content.len() + indent);
    let mut needs_prefix = true;
    let mut in_escape = false;
    for ch in content.chars() {
        if in_escape {
            out.push(ch);
            // CSI/SGR sequences end at the first ASCII letter; the
            // `~` is included for the rare 7-bit final byte some
            // sequences use. Good enough for syntect output.
            if ch.is_ascii_alphabetic() || ch == '~' {
                in_escape = false;
            }
        } else if ch == '\x1b' {
            out.push(ch);
            in_escape = true;
        } else if ch == '\n' {
            out.push(ch);
            needs_prefix = true;
        } else {
            if needs_prefix {
                out.push_str(&prefix);
                needs_prefix = false;
            }
            out.push(ch);
        }
    }
    out
}

fn formatter_from_config(config: &StyleConfig, pretty: bool) -> Formatter {
    let theme_name = if pretty {
        config.markdown.theme.as_deref()
    } else {
        None
    };
    if let Some(name) = theme_name
        && !theme::exists(name)
    {
        warn!("Unknown theme {name:?} in `style.markdown.theme`, falling back to the default.");
    }

    Formatter::with_width(config.markdown.wrap_width)
        .table_max_column_width(config.markdown.table_max_column_width)
        .theme(theme_name)
        .pretty_hr(pretty && config.markdown.hr_style.is_line())
        .inline_code_bg(
            config
                .inline_code
                .background
                .map(crate::format::color_to_bg_param),
        )
}

#[cfg(test)]
#[path = "chat_tests.rs"]
mod tests;
