use std::{env, fmt::Write as _, fs, sync::Arc, time, time::Duration};

use camino::{Utf8Path, Utf8PathBuf};
use crossterm::style::Stylize as _;
use jp_config::{
    conversation::tool::{
        ToolCommandConfig,
        style::{InlineResults, LinkStyle, ParametersStyle, TruncateLines},
    },
    style::StyleConfig,
};
use jp_conversation::event::ToolCallResponse;
use jp_llm::{CommandResult, run_tool_command};
use jp_md::format::Formatter;
use jp_printer::Printer;
use jp_term::osc::hyperlink;
use serde_json::{Map, Value};
use tokio::sync::mpsc::Sender;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::timer::spawn_tick_sender;

/// A tool in the pending list.
struct PendingTool {
    /// Tool call ID.
    id: String,
    /// Tool name.
    name: String,
}

/// Outcome of [`ToolRenderer::render_approved`].
#[derive(Debug, Clone)]
pub enum RenderOutcome {
    /// Header and arguments (if any) were printed.
    /// If a custom formatter produced output, it's returned for persistence.
    Rendered { content: Option<String> },
    /// Custom formatter failed — nothing was printed.
    Suppressed {
        /// Error message from the custom formatter.
        error: String,
    },
}

/// Renders tool-related output to the terminal and manages the streaming-phase
/// display for tool calls whose arguments are still being received.
///
/// The renderer owns the full tool display lifecycle:
///
/// 1. **Streaming phase** — a rewritable "temp line" shows tool names while
///    arguments are streamed. Once arguments are complete, a permanent line is
///    printed via [`complete`]. Methods: [`register`], [`complete`], [`tick`].
///
/// 2. **Permission/execution phase** — arguments (if not already rendered),
///    Custom formatter output, progress, and results. Methods:
///    [`render_tool_call`], [`render_approved`], [`render_progress`],
///    [`render_result`].
///
/// [`complete`]: Self::complete
/// [`register`]: Self::register
/// [`tick`]: Self::tick
/// [`render_tool_call`]: Self::render_tool_call
/// [`render_approved`]: Self::render_approved
/// [`render_progress`]: Self::render_progress
/// [`render_result`]: Self::render_result
pub struct ToolRenderer {
    printer: Arc<Printer>,
    config: StyleConfig,
    root: Utf8PathBuf,

    /// Markdown formatter used for syntax highlighting code blocks in tool
    /// results.
    formatter: Formatter,

    /// Tools not yet permanently displayed, in registration order.
    pending: Vec<PendingTool>,

    /// Whether a temp line is currently on screen.
    line_active: bool,

    /// Whether we're running in a TTY (controls timer spawning).
    is_tty: bool,

    /// Cancellation token for the tick timer task.
    timer_token: Option<CancellationToken>,
}

impl ToolRenderer {
    pub fn new(
        printer: Arc<Printer>,
        config: StyleConfig,
        root: Utf8PathBuf,
        is_tty: bool,
    ) -> Self {
        let formatter = Formatter::new().theme(if printer.pretty_printing_enabled() {
            config.markdown.theme.as_deref()
        } else {
            None
        });

        Self {
            printer,
            config,
            root,
            formatter,
            pending: Vec::new(),
            line_active: false,
            is_tty,
            timer_token: None,
        }
    }

    /// Renders header + arguments for a tool call.
    ///
    /// For `Custom` style, arguments are deferred to after approval via
    /// [`Self::render_approved`] — only the header is printed here.
    pub fn render_tool_call(
        &self,
        name: &str,
        arguments: &Map<String, Value>,
        style: &ParametersStyle,
    ) {
        let styled_name = name.yellow().bold();
        let args = format_args(arguments, style);

        let _ = writeln!(
            self.printer.err_writer(),
            "Calling tool {styled_name}{args}"
        );
    }

    /// Renders a tool call with all styles, printing header and arguments
    /// atomically.
    ///
    /// For non-Custom styles: prints the header with inline-formatted arguments
    /// in a single write.
    ///
    /// For Custom style: runs the custom formatter command first, then prints
    /// the header followed by the formatted output. If the custom formatter
    /// fails, nothing is printed and [`RenderOutcome::Suppressed`] is returned.
    ///
    /// On success, returns `Rendered { content }` where `content` is the
    /// custom-formatted output (if any) so the caller can persist it for replay.
    pub async fn render_approved(
        &self,
        name: &str,
        arguments: &Map<String, Value>,
        style: &ParametersStyle,
    ) -> RenderOutcome {
        if let ParametersStyle::Custom(cmd_config) = style {
            let cmd = cmd_config.clone().command();
            self.render_custom_tool_call(name, arguments, cmd).await
        } else {
            self.render_tool_call(name, arguments, style);
            RenderOutcome::Rendered { content: None }
        }
    }

    /// Renders a Custom-style tool call: header + custom formatted output.
    ///
    /// Runs the custom formatter command first. If it succeeds, prints the
    /// "Calling tool X" header followed by the formatted output. If it fails,
    /// nothing is printed — the tool call is suppressed from the display.
    async fn render_custom_tool_call(
        &self,
        name: &str,
        arguments: &Map<String, Value>,
        cmd: ToolCommandConfig,
    ) -> RenderOutcome {
        match format_args_custom(name, arguments, cmd, &self.root).await {
            Ok(content) if !content.is_empty() => {
                let styled_name = name.yellow().bold();
                let _ = writeln!(self.printer.err_writer(), "Calling tool {styled_name}");
                self.render_formatted_arguments(&content);
                RenderOutcome::Rendered {
                    content: Some(content),
                }
            }
            Ok(_) => {
                // Custom formatter returned empty — just show the header.
                let styled_name = name.yellow().bold();
                let _ = writeln!(self.printer.err_writer(), "Calling tool {styled_name}");
                RenderOutcome::Rendered { content: None }
            }
            Err(error) => {
                warn!(%error, tool = %name, "Custom formatter failed, suppressing tool call display");
                RenderOutcome::Suppressed { error }
            }
        }
    }

    /// Render already-formatted custom argument content.
    ///
    /// Used by [`render_approved`](Self::render_approved) internally and by
    /// the replay path when the stored event has rendered arguments in its
    /// metadata.
    pub fn render_formatted_arguments(&self, content: &str) {
        let _ = writeln!(self.printer.err_writer(), "\n{content}");
        if !content.ends_with("\n\n") {
            let _ = writeln!(self.printer.err_writer());
        }
    }

    /// Renders elapsed time for a long-running tool.
    pub fn render_progress(&self, elapsed: Duration) {
        let secs = elapsed.as_secs_f64();
        let _ = write!(self.printer.err_writer(), "\r\x1b[K⏱ Running… {secs:.1}s");
    }

    /// Clears the current progress line.
    pub fn clear_progress(&self) {
        // Carriage return + ANSI escape to clear to end of line
        let _ = write!(self.printer.err_writer(), "\r\x1b[K");
    }

    /// Returns the progress configuration.
    pub fn progress_config(&self) -> &jp_config::style::tool_call::ProgressConfig {
        &self.config.tool_call.progress
    }

    /// Renders a tool call result with language detection, truncation, and file links.
    ///
    /// This method handles the full result rendering flow:
    /// 1. Parses content to detect if it's JSON and pretty-prints it
    /// 2. Detects language from code fences or content inspection (XML/JSON)
    /// 3. Writes the full content to a temp file for linking
    /// 4. Truncates the displayed output based on `inline_results` config
    /// 5. Renders file links based on `results_file_link` config
    ///
    /// # Arguments
    ///
    /// * `response` - The tool call response containing the result
    /// * `inline_results` - How to display inline results (Off, Full, Truncate)
    /// * `results_file_link` - How to display file links (Off, Full, Osc8)
    #[allow(clippy::too_many_lines)]
    pub fn render_result(
        &self,
        response: &ToolCallResponse,
        inline_results: &InlineResults,
        results_file_link: &LinkStyle,
    ) {
        // Skip rendering if inline results are off and no file link
        if matches!(inline_results, InlineResults::Off)
            && matches!(results_file_link, LinkStyle::Off)
        {
            return;
        }

        // Get content, handling both Ok and Err results
        let raw_content = response.content();

        // Try to parse as JSON and pretty-print if valid
        let content = if let Ok(json) = serde_json::from_str::<Value>(raw_content.trim()) {
            if let Ok(pretty) = serde_json::to_string_pretty(&json) {
                format!("```json\n{pretty}\n```")
            } else {
                raw_content.trim_end().to_owned()
            }
        } else {
            raw_content.trim_end().to_owned()
        };

        // Extract language from code fence if present
        let mut lines: Vec<&str> = content.lines().collect();
        let mut ext = lines.first().and_then(|v| v.strip_prefix("```")).map(|v| {
            v.chars()
                .take_while(char::is_ascii_alphabetic)
                .collect::<String>()
        });

        // Remove code fence markers for processing
        if ext.is_some() && !lines.is_empty() {
            lines.remove(0);
            lines.pop_if(|v| v.trim() == "```");
        }

        if ext.is_none() {
            let trimmed = content.trim();
            if trimmed.starts_with('<') && quick_xml::de::from_str::<Value>(trimmed).is_ok() {
                ext = Some("xml".to_owned());
            } else if trimmed.starts_with('{') && serde_json::from_str::<Value>(trimmed).is_ok() {
                ext = Some("json".to_owned());
            }
        }

        let inner_content = lines.join("\n");

        // Write to temp file
        let millis = time::SystemTime::now()
            .duration_since(time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_millis();

        let file_name = match ext.as_ref() {
            Some(e) if !e.is_empty() => format!("tool_call_{millis}.{e}"),
            _ => format!("tool_call_{millis}"),
        };

        let path = env::temp_dir().join(&file_name);
        let _err = fs::write(&path, &inner_content);

        // Determine max lines based on config
        let total_lines = inner_content.lines().count();
        let max_lines = match inline_results {
            InlineResults::Off => 0,
            InlineResults::Full => total_lines,
            InlineResults::Truncate(TruncateLines { lines }) => *lines,
        };

        // Render intro header
        if !matches!(inline_results, InlineResults::Off) && max_lines > 0 {
            let lang = ext.as_ref().filter(|e| !e.is_empty());
            let mut code_state = lang.map(|lang| self.formatter.begin_code_block(lang));
            let mut output = "\n".to_owned();

            if let Some(lang) = ext.as_ref() {
                output.push_str("```");
                output.push_str(lang);
                output.push('\n');
            }

            for line in inner_content.lines().take(max_lines) {
                // highlight_line expects the trailing newline.
                let with_nl = format!("{line}\n");
                if let Some(ref mut state) = code_state {
                    let rendered = self.formatter.render_code_line(&with_nl, state, None);
                    output.push_str(&rendered);
                } else {
                    output.push_str(line);
                    output.push('\n');
                }
            }

            if ext.is_some() {
                output.push_str("```");
            }

            if !output.ends_with('\n') {
                output.push('\n');
            }

            if inline_results.is_truncated() && max_lines < total_lines {
                output.push_str(&format!(" _(truncated to {max_lines} lines)_"));
            }

            let _ = write!(self.printer.err_writer(), "{output}");
        }

        // Render file links
        match results_file_link {
            LinkStyle::Off => {}
            LinkStyle::Full => {
                let _ = writeln!(self.printer.err_writer(), "see: {}\n", path.display());
            }
            LinkStyle::Osc8 => {
                let _ = writeln!(
                    self.printer.err_writer(),
                    "[{}] [{}]\n",
                    hyperlink(
                        format!("file://{}", path.display()),
                        "open in editor".red().to_string()
                    ),
                    hyperlink(
                        format!("copy://{}", path.display()),
                        "copy to clipboard".red().to_string()
                    )
                );
            }
        }
    }

    /// Registers a new tool call (name known, arguments pending).
    ///
    /// Adds the tool to the rewritable temp line. Starts the tick timer on the
    /// first registration.
    pub fn register(&mut self, id: &str, name: &str, tick_tx: &Sender<Duration>) {
        if self.pending.iter().any(|t| t.id == id) {
            return;
        }

        self.pending.push(PendingTool {
            id: id.to_owned(),
            name: name.to_owned(),
        });

        if self.line_active {
            self.rewrite_temp_line();
        } else {
            self.write_temp_line();
            self.line_active = true;
        }

        self.ensure_timer(tick_tx);
    }

    /// Completes a tool call and removes it from the temp line.
    ///
    /// This only handles the rewritable temp-line display. The permanent
    /// "Calling tool ..." header is printed later by [`render_approved`]
    /// after the permission decision.
    ///
    /// [`render_approved`]: Self::render_approved
    pub fn complete(&mut self, id: &str) {
        self.pending.retain(|t| t.id != id);

        if self.line_active {
            let _ = write!(self.printer.err_writer(), "\r\x1b[K");
        }

        if self.pending.is_empty() {
            self.line_active = false;
        } else {
            self.write_temp_line();
        }
    }

    /// Updates the temp line with the elapsed time.
    ///
    /// Called on each tick from the timer task.
    pub fn tick(&self, elapsed: Duration) {
        if self.pending.is_empty() {
            return;
        }

        let content = self.temp_line_content();
        let secs = elapsed.as_secs_f64();
        let _ = write!(
            self.printer.err_writer(),
            "\r\x1b[K{content} (receiving arguments… {secs:.1}s)",
        );
    }

    /// Returns `true` if there are tools waiting for arguments.
    #[allow(dead_code)]
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Clears the temp line visually without modifying state.
    ///
    /// Used before showing interrupt menus. The next tick will
    /// redisplay the temp line.
    pub fn clear_temp_line(&self) {
        if self.pending.is_empty() {
            return;
        }
        let _ = write!(self.printer.err_writer(), "\r\x1b[K");
    }

    /// Clears the temp line and all pending state. Stops the timer.
    pub fn cancel_all(&mut self) {
        self.stop_timer();

        if self.line_active {
            let _ = write!(self.printer.err_writer(), "\r\x1b[K");
            self.line_active = false;
        }

        self.pending.clear();
    }

    /// Resets all state for a new streaming cycle.
    pub fn reset(&mut self) {
        self.line_active = false;
        self.cancel_all();
    }

    fn temp_line_content(&self) -> String {
        let label = if self.pending.len() == 1 {
            "tool"
        } else {
            "tools"
        };

        let names: Vec<_> = self
            .pending
            .iter()
            .map(|t| t.name.as_str().yellow().bold().to_string())
            .collect();

        format!("Calling {label} {}", names.join(", "))
    }

    fn write_temp_line(&self) {
        let content = self.temp_line_content();
        let _ = write!(self.printer.err_writer(), "{content}");
    }

    fn rewrite_temp_line(&self) {
        let content = self.temp_line_content();
        let _ = write!(self.printer.err_writer(), "\r{content}\x1b[K");
    }

    fn ensure_timer(&mut self, tick_tx: &Sender<Duration>) {
        if self.timer_token.is_some() || !self.is_tty {
            return;
        }

        let preparing = &self.config.tool_call.preparing;
        self.timer_token = spawn_tick_sender(
            tick_tx.clone(),
            preparing.show,
            Duration::from_secs(u64::from(preparing.delay_secs)),
            Duration::from_millis(u64::from(preparing.interval_ms)),
        );
    }

    fn stop_timer(&mut self) {
        if let Some(token) = self.timer_token.take() {
            token.cancel();
        }
    }
}

/// Formats tool call arguments for display based on the configured style.
///
/// Arguments with empty values (`{}`, `[]`, `null`) are stripped before
/// formatting.
///
/// - `Off` / `Custom` → `""` (Custom content is rendered separately)
/// - `Json` → JSON block with arguments
/// - `FunctionCall` → `(key=value, ...)`
fn format_args(arguments: &Map<String, Value>, style: &ParametersStyle) -> String {
    let filtered = filter_display_args(arguments);

    if filtered.is_empty() {
        return String::new();
    }

    match style {
        // Off and Custom produce no inline output.
        // Custom content is rendered separately via render_approved.
        ParametersStyle::Off | ParametersStyle::Custom(_) => String::new(),

        ParametersStyle::Json => format_args_json(filtered),

        ParametersStyle::FunctionCall => {
            let mut buf = String::new();
            buf.push('(');
            for (i, (key, value)) in filtered.iter().enumerate() {
                if i > 0 {
                    buf.push_str(", ");
                }
                let dim_key = key.clone().dim();
                buf.push_str(&format!("{dim_key}: {value}"));
            }
            buf.push(')');
            buf
        }
    }
}

/// Filters out visually empty arguments before display.
fn filter_display_args(arguments: &Map<String, Value>) -> Map<String, Value> {
    arguments
        .iter()
        .filter(|(_, value)| !is_display_empty(value))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

fn is_display_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Object(m) => m.is_empty(),
        Value::Array(a) => a.is_empty(),
        _ => false,
    }
}

/// Render a JSON representation of the arguments.
fn format_args_json(arguments: Map<String, Value>) -> String {
    let pretty = format!("{:#}", Value::Object(arguments));
    format!(" with arguments:\n\n```json\n{pretty}\n```")
}

/// Runs a custom arguments formatter command and returns the content.
async fn format_args_custom(
    tool_name: &str,
    arguments: &Map<String, Value>,
    cmd: ToolCommandConfig,
    root: &Utf8Path,
) -> Result<String, String> {
    let ctx = serde_json::json!({
        "tool": {
            "name": tool_name,
            "arguments": arguments,
        },
        "context": {
            "action": jp_tool::Action::FormatArguments,
            "root": root,
        },
    });

    let result = run_tool_command(cmd.clone(), ctx, root, CancellationToken::new(), None)
        .await
        .map_err(|e| {
            warn!(
                command = %cmd,
                error = %e,
                "Custom parameters formatter failed"
            );
            format!("Custom parameters formatter '{cmd}' failed: {e}")
        })?;

    match result {
        CommandResult::Success(content) => Ok(content.trim().to_owned()),
        CommandResult::TransientError { message, trace } => {
            let detail = CommandResult::format_error(&message, &trace);
            warn!(
                command = %cmd,
                error = %detail,
                "Custom parameters formatter returned error"
            );
            Err(detail)
        }
        CommandResult::FatalError(raw) => {
            warn!(
                command = %cmd,
                "Custom parameters formatter returned fatal error"
            );
            Err(raw)
        }
        CommandResult::NeedsInput(_) => {
            warn!(
                command = %cmd,
                "Custom parameters formatter returned NeedsInput"
            );
            Err(format!(
                "Custom parameters formatter '{cmd}' returned unexpected NeedsInput"
            ))
        }
        CommandResult::Cancelled => Ok(String::new()),
        CommandResult::RawOutput {
            stdout,
            success: true,
            ..
        } => Ok(stdout.trim().to_owned()),
        CommandResult::RawOutput { stderr, .. } => {
            warn!(
                command = %cmd,
                error = %stderr,
                "Custom parameters formatter failed"
            );
            Err(format!(
                "Custom parameters formatter '{cmd}' failed: {stderr}"
            ))
        }
    }
}

#[cfg(test)]
#[path = "tool_tests.rs"]
mod tests;
