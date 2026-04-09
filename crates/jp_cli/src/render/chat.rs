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

use std::sync::Arc;

use jp_config::style::{
    StyleConfig,
    reasoning::{ReasoningDisplayConfig, TruncateChars},
};
use jp_conversation::event::ChatResponse;
use jp_md::{
    buffer::{Buffer, Event},
    format::{BackgroundFill, DefaultBackground, Formatter, SavedHighlightState, TerminalOptions},
};
use jp_printer::{PrintableExt as _, Printer};

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
    /// Saved highlighting state for the current fenced code block, if any.
    code_highlight: Option<SavedHighlightState>,
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
            code_highlight: None,
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
            ReasoningDisplayConfig::Hidden => {}

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
                    self.flush();
                    self.last_content_kind = Some(ContentKind::Reasoning);
                    self.printer.eprint("reasoning...");
                }
            }

            ReasoningDisplayConfig::Static => {
                if self.last_content_kind != Some(ContentKind::Reasoning) {
                    self.flush();
                    self.last_content_kind = Some(ContentKind::Reasoning);
                    self.printer.eprintln("reasoning...\n");
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
        while let Some(event) = self.buffer.next() {
            match event {
                Event::Block(text) | Event::Flush(text) => self.print_block(&text),
                Event::FencedCodeStart { ref language, .. } => {
                    self.begin_code_block(language);
                    self.print_code(&format!("{event}\n"));
                }
                Event::FencedCodeLine(line) => {
                    let highlighted = self.highlight_code_line(&line);
                    self.print_code(&highlighted);
                }
                Event::FencedCodeEnd(fence) => {
                    self.print_code(&format!("{fence}\n"));
                    self.end_code_block();
                }
            }
        }
    }

    /// Set up syntax highlighting state for a new fenced code block.
    fn begin_code_block(&mut self, language: &str) {
        self.code_highlight = self
            .formatter
            .new_code_highlighter(language)
            .map(jp_md::format::CodeHighlighter::save);
    }

    /// Highlight a single code line using the saved highlighting state.
    ///
    /// Reconstructs the highlighter from saved state, highlights the line,
    /// then saves the updated state back. Falls back to the raw line if
    /// no highlighting state is available or highlighting fails.
    fn highlight_code_line(&mut self, line: &str) -> String {
        let Some(saved) = self.code_highlight.take() else {
            return line.to_string();
        };

        let mut hl = self.formatter.resume_code_highlighter(saved);
        let result = hl.highlight(line);
        self.code_highlight = Some(hl.save());
        result.unwrap_or_else(|_| line.to_string())
    }

    /// Clean up after a fenced code block ends.
    fn end_code_block(&mut self) {
        self.code_highlight = None;
    }

    /// Print a raw code string with the code typewriter delay.
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
        let mut formatted = self
            .formatter
            .format_terminal_with(block, &opts)
            .unwrap_or_else(|_| block.to_string());

        // The trailing newline creates the blank line between blocks.
        // When a default background is active, fill the blank line too
        // so the background is continuous across paragraphs.
        match opts.default_background {
            Some(DefaultBackground {
                ref param,
                fill: BackgroundFill::Terminal,
            }) => {
                formatted.push_str(&format!("\x1b[{param}m\x1b[K\x1b[49m\n"));
            }
            Some(DefaultBackground {
                ref param,
                fill: BackgroundFill::Column(width),
            }) => {
                formatted.push_str(&format!("\x1b[{param}m"));
                for _ in 0..width {
                    formatted.push(' ');
                }
                formatted.push_str("\x1b[49m\n");
            }
            _ => formatted.push('\n'),
        }

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
        // If we're mid-code-block, the stream ended without a closing fence.
        // Emit what we have as raw text.
        if self.code_highlight.is_some() {
            self.end_code_block();
        }
        if let Some(remaining) = self.buffer.flush() {
            self.print_block(&remaining);
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
        self.buffer = Buffer::new();
        let pretty = self.printer.pretty_printing_enabled();
        self.formatter = formatter_from_config(&self.config, pretty);
        self.last_content_kind = None;
        self.reasoning_chars_count = 0;
        self.code_highlight = None;
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
