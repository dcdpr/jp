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
//! | Mode | Behavior |
//! |------|----------|
//! | `Hidden` | Don't render reasoning (still persisted) |
//! | `Full` | Render all reasoning tokens |
//! | `Truncate(N)` | Render first N characters, then "..." |
//! | `Progress` | Show "reasoning..." then dots |
//! | `Static` | Show "reasoning..." once |
//! | `Timer` | Show a running timer, erase when done |

use std::{fmt::Write as _, sync::Arc, time::Duration};

use jp_config::style::{
    StyleConfig,
    reasoning::{ReasoningDisplayConfig, TruncateChars},
};
use jp_conversation::event::ChatResponse;
use jp_md::{
    buffer::{Buffer, Event, EventFixup, FenceEscalationFixup, OrphanedFenceFixup},
    format::{BackgroundFill, CodeBlockState, DefaultBackground, Formatter, TerminalOptions},
};
use jp_printer::{PrintableExt as _, Printer};
use tokio_util::sync::CancellationToken;

use crate::timer::spawn_line_timer;

/// The kind of content last pushed into the renderer.
///
/// Used to detect content-type transitions so that the markdown buffer
/// can be force-flushed before a different kind of content is rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContentKind {
    Reasoning,
    Message,
    ToolCall,
}

/// Renders chat events to the terminal.
///
/// Handles user messages, assistant reasoning, and assistant message content,
/// applying the configured display mode for reasoning. Tracks content-kind
/// transitions to insert appropriate spacing between different content types
/// (e.g. blank lines between tool calls and message text).
pub struct ChatRenderer {
    buffer: Buffer,
    formatter: Formatter,
    printer: Arc<Printer>,
    config: StyleConfig,
    last_content_kind: Option<ContentKind>,
    reasoning_chars_count: usize,
    /// State for the current streaming fenced code block, if any.
    code_block: Option<CodeBlockState>,
    /// Active reasoning timer token, used by `Timer` display mode.
    reasoning_timer: Option<CancellationToken>,
    /// Post-processing fixups for LLM quirks in the event stream.
    fixups: Vec<Box<dyn EventFixup>>,
}

impl ChatRenderer {
    pub fn new(printer: Arc<Printer>, config: StyleConfig) -> Self {
        let pretty = printer.pretty_printing_enabled();
        let formatter = formatter_from_config(&config, pretty);
        Self {
            buffer: Buffer::new(),
            formatter,
            printer,
            config,
            last_content_kind: None,
            reasoning_chars_count: 0,
            code_block: None,
            reasoning_timer: None,
            fixups: vec![
                Box::new(OrphanedFenceFixup::new()),
                Box::new(FenceEscalationFixup),
            ],
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
    /// Formats the content with a horizontal rule separator and prints it
    /// as a complete block. Participates in content-kind transition tracking
    /// so that spacing between user messages and tool calls is correct.
    pub fn render_request(&mut self, content: &str) {
        self.flush_on_transition(ContentKind::Message);
        self.flush();

        let formatted = self
            .formatter
            .format_terminal(content.trim_end())
            .unwrap_or_else(|_| content.trim_end().to_owned());
        self.printer.print(&formatted);

        self.render_separator();

        self.last_content_kind = Some(ContentKind::Message);
    }

    /// Render a horizontal rule separator between turns.
    ///
    /// Routes the `---` through the markdown formatter so it renders as
    /// a proper HR element in pretty mode.
    pub fn render_separator(&mut self) {
        self.flush();

        let formatted = self
            .formatter
            .format_terminal("\n\n---\n\n")
            .unwrap_or_else(|_| "\n\n---\n\n".to_owned());
        self.printer.print(format!("\n{formatted}\n"));

        self.last_content_kind = None;
    }

    /// Flush the markdown buffer if the content kind is changing.
    ///
    /// When the LLM switches from one content type to another (e.g.
    /// reasoning → message, or message → tool call), any partial markdown
    /// sitting in the buffer must be emitted immediately. Without this,
    /// content before the transition would only appear after the next
    /// block boundary — which may not arrive until much later (or never,
    /// if a tool call follows).
    fn flush_on_transition(&mut self, next: ContentKind) {
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
                if self.last_content_kind != Some(ContentKind::Reasoning) {
                    self.flush_on_transition(ContentKind::Reasoning);

                    if let Some((token, _handle)) = spawn_line_timer(
                        self.printer.clone(),
                        self.printer.pretty_printing_enabled(),
                        Duration::from_millis(300),
                        Duration::from_millis(100),
                        |secs| {
                            format!("\r\x1b[K\x1b[2m\u{23f1} Reasoning\u{2026} {secs:.1}s\x1b[22m")
                        },
                    ) {
                        self.reasoning_timer = Some(token);
                    }
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
            let Some(event) = self.apply_fixups(raw_event) else {
                continue;
            };
            match event {
                Event::Block(text) | Event::Flush(text) => self.print_block(&text),
                Event::FencedCodeStart { ref language, .. } => {
                    self.code_block = Some(self.formatter.begin_code_block(language));
                    let bg = self.terminal_options().default_background;
                    let rendered = self
                        .formatter
                        .render_code_fence(&format!("{event}\n"), bg.as_ref());
                    self.print_code(&rendered);
                }
                Event::FencedCodeLine(line) => {
                    let bg = self.terminal_options().default_background;
                    let rendered = if let Some(ref mut state) = self.code_block {
                        self.formatter.render_code_line(&line, state, bg.as_ref())
                    } else {
                        line
                    };
                    self.print_code(&rendered);
                }
                Event::FencedCodeEnd(fence) => {
                    let bg = self.terminal_options().default_background;
                    let rendered = self
                        .formatter
                        .render_closing_fence(&format!("{fence}\n"), bg.as_ref());
                    self.print_code(&rendered);
                    self.code_block = None;
                }
            }
        }
    }

    /// Print a raw code string with the code typewriter delay.
    ///
    /// The content is already highlighted and has background applied by
    /// the formatter's streaming code block API.
    fn print_code(&self, content: &str) {
        let delay = self.config.typewriter.code_delay;
        self.printer
            .print(content.to_string().typewriter(delay.into()));
    }

    fn print_block(&self, block: &str) {
        // Skip whitespace-only blocks. These can appear when the LLM emits
        // blank text content blocks (e.g. "\n\n" between interleaved thinking
        // blocks) that survive a buffer flush.
        if block.trim().is_empty() {
            return;
        }

        let opts = self.terminal_options();
        let formatted = self
            .formatter
            .format_terminal_with(block, &opts)
            .unwrap_or_else(|_| block.to_string());

        let delay = self.config.typewriter.text_delay;
        self.printer.print(formatted.typewriter(delay.into()));
    }

    /// Build per-block terminal options based on the current content kind.
    fn terminal_options(&self) -> TerminalOptions {
        TerminalOptions {
            default_background: if self.last_content_kind == Some(ContentKind::Reasoning) {
                self.config
                    .reasoning
                    .background
                    .map(|color| DefaultBackground {
                        param: crate::format::color_to_bg_param(color),
                        fill: BackgroundFill::Terminal,
                    })
            } else {
                None
            },
        }
    }

    pub fn flush(&mut self) {
        self.cancel_reasoning_timer();
        // If we're mid-code-block, the stream ended without a closing fence.
        // Emit what we have as raw text.
        self.code_block = None;
        if let Some(remaining) = self.buffer.flush() {
            self.print_block(&remaining);
        }
    }

    /// Cancel the reasoning timer if one is running.
    ///
    /// Cancels the token (so the background task stops ticking) and
    /// clears the timer line on stderr immediately.
    fn cancel_reasoning_timer(&mut self) {
        if let Some(token) = self.reasoning_timer.take() {
            token.cancel();
            let _ = write!(self.printer.err_writer(), "\r\x1b[K");
        }
    }

    /// Transition renderer state to tool call mode.
    pub fn transition_to_tool_call(&mut self) {
        self.last_content_kind = Some(ContentKind::ToolCall);
    }

    /// Reset the renderer state, discarding any buffered content.
    ///
    /// Used when the current streaming cycle is being interrupted and a new
    /// one will start (e.g., after a Reply or Continue action). The partial
    /// content in the buffer has already been captured by the event builder,
    /// so it's safe to discard.
    pub fn reset(&mut self) {
        self.cancel_reasoning_timer();
        self.buffer = Buffer::new();
        let pretty = self.printer.pretty_printing_enabled();
        self.formatter = formatter_from_config(&self.config, pretty);
        self.last_content_kind = None;
        self.reasoning_chars_count = 0;
        self.code_block = None;
        self.fixups = vec![
            Box::new(OrphanedFenceFixup::new()),
            Box::new(FenceEscalationFixup),
        ];
    }

    /// Run the event through all fixups. Returns `None` if suppressed.
    fn apply_fixups(&mut self, event: Event) -> Option<Event> {
        self.fixups
            .iter_mut()
            .try_fold(event, |ev, fixup| fixup.process(ev).ok_or(()))
            .ok()
    }
}

fn formatter_from_config(config: &StyleConfig, pretty: bool) -> Formatter {
    Formatter::with_width(config.markdown.wrap_width)
        .table_max_column_width(config.markdown.table_max_column_width)
        .theme(if pretty {
            config.markdown.theme.as_deref()
        } else {
            None
        })
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
