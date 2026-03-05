use std::{collections::HashSet, env, fmt::Write as _, fs, sync::Arc, time, time::Duration};

use camino::{Utf8Path, Utf8PathBuf};
use crossterm::style::Stylize as _;
use jp_config::{
    conversation::tool::style::{InlineResults, LinkStyle, ParametersStyle, TruncateLines},
    style::StyleConfig,
};
use jp_conversation::event::ToolCallResponse;
use jp_printer::Printer;
use jp_term::osc::hyperlink;
use serde_json::Value;
use tokio::{
    sync::mpsc::Sender,
    time::{Instant, MissedTickBehavior},
};
use tokio_util::sync::CancellationToken;
use tracing::warn;

/// Spawns a timer task that sends elapsed [`Duration`] through a channel
/// at a fixed interval.
///
/// After `delay`, the task sends its elapsed time every `interval`. On
/// cancellation (or when the receiver is dropped), the task exits.
///
/// Returns `None` if `show` is `false`, in which case nothing is spawned.
pub fn spawn_tick_sender(
    tx: Sender<Duration>,
    show: bool,
    delay: Duration,
    interval: Duration,
) -> Option<CancellationToken> {
    if !show {
        return None;
    }

    let token = CancellationToken::new();
    let child = token.child_token();
    let interval = interval.max(Duration::from_millis(10));

    tokio::spawn(async move {
        let start = Instant::now();

        tokio::select! {
            () = tokio::time::sleep(delay) => {}
            () = child.cancelled() => { return; }
        }

        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                biased;
                () = child.cancelled() => { return; }
                _ = ticker.tick() => {
                    if tx.send(start.elapsed()).await.is_err() {
                        return;
                    }
                }
            }
        }
    });

    Some(token)
}

/// Spawns a `\r`-based timer task that periodically writes a status line.
///
/// After `delay`, the task calls `format_line(elapsed_secs)` every
/// `interval` and writes the result to the printer. On cancellation it
/// clears the line with `\r\x1b[K`.
///
/// Returns `None` if `show` is `false`, in which case nothing is spawned.
pub fn spawn_line_timer(
    printer: Arc<Printer>,
    show: bool,
    delay: Duration,
    interval: Duration,
    format_line: impl Fn(f64) -> String + Send + 'static,
) -> Option<(CancellationToken, tokio::task::JoinHandle<()>)> {
    if !show {
        return None;
    }

    let token = CancellationToken::new();
    let child = token.child_token();
    let interval = interval.max(Duration::from_millis(10));

    let handle = tokio::spawn(async move {
        let start = Instant::now();

        tokio::select! {
            () = tokio::time::sleep(delay) => {}
            () = child.cancelled() => { return; }
        }

        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                biased;
                () = child.cancelled() => {
                    let _ = write!(printer.out_writer(), "\r\x1b[K");
                    return;
                }
                _ = ticker.tick() => {
                    let secs = start.elapsed().as_secs_f64();
                    let line = format_line(secs);
                    let _ = write!(printer.out_writer(), "{line}");
                }
            }
        }
    });

    Some((token, handle))
}
/// Result of formatting a tool call's arguments.
#[derive(Debug)]
pub struct FormatResult {
    /// Tool call ID.
    pub id: String,
    /// Tool name.
    pub name: String,
    /// Formatted arguments string (to append after header), or error.
    pub formatted: Result<String, String>,
}

/// A tool in the pending list.
struct PendingTool {
    /// Tool call ID.
    id: String,
    /// Tool name.
    name: String,
}

/// Renders tool-related output to the terminal and manages the streaming-phase
/// display for tool calls whose arguments are still being received.
///
/// The renderer owns the full tool display lifecycle:
///
/// 1. **Streaming phase** — a rewritable "temp line" shows tool names while
///    arguments are streamed. Methods: [`register`](Self::register),
///    [`complete`](Self::complete), [`tick`](Self::tick).
///
/// 2. **Execution phase** — permanent headers, progress, and results.
///    Methods: [`render_call_header`](Self::render_call_header),
///    [`render_arguments`](Self::render_arguments),
///    [`render_progress`](Self::render_progress),
///    [`render_result`](Self::render_result).
pub struct ToolRenderer {
    printer: Arc<Printer>,
    config: StyleConfig,
    root: Utf8PathBuf,

    /// Tools not yet permanently displayed, in registration order.
    pending: Vec<PendingTool>,
    /// Tool IDs whose header+args have been permanently printed.
    rendered: HashSet<String>,
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
        Self {
            printer,
            config,
            root,
            pending: Vec::new(),
            rendered: HashSet::new(),
            line_active: false,
            is_tty,
            timer_token: None,
        }
    }

    /// Renders just the "Calling tool X" header (without arguments or
    /// newline).
    ///
    /// Used during the streaming phase when the tool name is known but
    /// arguments are still being received. The line is left open so the
    /// preparing timer can append "(receiving arguments… Ns)" on the same
    /// line, and `render_arguments` can later replace it with the full
    /// styled arguments.
    pub fn render_call_header(&self, name: &str) {
        let styled_name = name.yellow().bold();
        let _ = write!(self.printer.out_writer(), "\nCalling tool {styled_name}");
    }

    /// Renders the formatted arguments after the "Calling tool X" header.
    ///
    /// The header is always printed by [`render_call_header`] first (either
    /// during the streaming phase for multi-part tool calls, or right before
    /// this method for single-part tool calls). This method appends the
    /// styled arguments and a trailing newline.
    ///
    /// Returns `Err` if a `Custom` style command fails; the caller should
    /// use the error to short-circuit tool execution.
    ///
    /// [`render_call_header`]: Self::render_call_header
    pub async fn render_arguments(
        &self,
        name: &str,
        arguments: &serde_json::Map<String, Value>,
        style: &ParametersStyle,
    ) -> Result<(), String> {
        let args = format_args(name, arguments, style, &self.root).await?;
        let _ = writeln!(self.printer.out_writer(), "{args}");

        Ok(())
    }

    /// Renders elapsed time for a long-running tool.
    pub fn render_progress(&self, elapsed: Duration) {
        let secs = elapsed.as_secs_f64();
        let _ = write!(self.printer.out_writer(), "\r\x1b[K⏱ Running… {secs:.1}s");
    }

    /// Clears the current progress line.
    pub fn clear_progress(&self) {
        // Carriage return + ANSI escape to clear to end of line
        let _ = write!(self.printer.out_writer(), "\r\x1b[K");
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
                raw_content.trim().to_owned()
            }
        } else {
            raw_content.trim().to_owned()
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
        }
        if lines.last().is_some_and(|v| v.trim() == "```") {
            lines.pop();
        }

        // Auto-detect language from content if not already set
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
        drop(fs::write(&path, &inner_content));

        // Determine max lines based on config
        let total_lines = inner_content.lines().count();
        let max_lines = match inline_results {
            InlineResults::Off => 0,
            InlineResults::Full => total_lines,
            InlineResults::Truncate(TruncateLines { lines }) => *lines,
        };

        // Render intro header
        if !matches!(inline_results, InlineResults::Off) {
            let mut intro = "\nTool call result".to_owned();
            if let InlineResults::Truncate(TruncateLines { lines }) = inline_results
                && *lines < total_lines
            {
                intro.push_str(&format!(" _(truncated to {lines} lines)_"));
            }
            intro.push_str(":\n");
            let _ = write!(self.printer.out_writer(), "{intro}");
        }

        // Render content with code fence if we have a language
        if !matches!(inline_results, InlineResults::Off) && max_lines > 0 {
            let mut output = "\n".to_owned();

            if let Some(e) = ext.as_ref()
                && !e.is_empty()
            {
                output.push_str("```");
                output.push_str(e);
                output.push('\n');
            }

            for line in inner_content.lines().take(max_lines) {
                output.push_str(line);
                output.push('\n');
            }

            if ext.as_ref().is_some_and(|e| !e.is_empty()) {
                output.push_str("```");
            }

            if !output.ends_with('\n') {
                output.push('\n');
            }

            let _ = write!(self.printer.out_writer(), "{output}");
        }

        // Render file links
        match results_file_link {
            LinkStyle::Off => {}
            LinkStyle::Full => {
                let _ = writeln!(self.printer.out_writer(), "see: {}\n", path.display());
            }
            LinkStyle::Osc8 => {
                let _ = writeln!(
                    self.printer.out_writer(),
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
    /// Adds the tool to the rewritable temp line. Starts the tick timer
    /// on the first registration.
    ///
    /// Hidden tools are immediately marked as rendered without any output.
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
            let _ = writeln!(self.printer.out_writer());
            self.write_temp_line();
            self.line_active = true;
        }

        self.ensure_timer(tick_tx);
    }

    /// Handles successful formatting. Prints a permanent line and removes
    /// the tool from the temp line.
    ///
    /// Hidden tools are silently marked as rendered without output.
    pub fn complete(&mut self, id: &str, name: &str, formatted_args: &str) {
        self.pending.retain(|t| t.id != id);

        if self.line_active {
            let _ = write!(self.printer.out_writer(), "\r\x1b[K");
        }

        let styled_name = name.yellow().bold();
        let _ = writeln!(
            self.printer.out_writer(),
            "Calling tool {styled_name}{formatted_args}",
        );

        self.rendered.insert(id.to_owned());

        if self.pending.is_empty() {
            self.line_active = false;
        } else {
            self.write_temp_line();
        }
    }

    /// Removes a tool from pending without printing a permanent line.
    ///
    /// Used when formatting fails — the tool will be rendered in the
    /// execution phase as a fallback.
    pub fn remove_pending(&mut self, id: &str) {
        self.pending.retain(|t| t.id != id);

        if self.pending.is_empty() {
            if self.line_active {
                let _ = write!(self.printer.out_writer(), "\r\x1b[K");
                self.line_active = false;
            }
        } else if self.line_active {
            self.rewrite_temp_line();
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
            self.printer.out_writer(),
            "\r\x1b[K{content} (receiving arguments… {secs:.1}s)",
        );
    }

    /// Returns `true` if the tool has been permanently rendered.
    pub fn is_rendered(&self, id: &str) -> bool {
        self.rendered.contains(id)
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
        let _ = write!(self.printer.out_writer(), "\r\x1b[K");
    }

    /// Clears the temp line and all pending state. Stops the timer.
    ///
    /// The `rendered` set is preserved — it's needed by the execution
    /// phase to skip already-displayed tools.
    pub fn cancel_all(&mut self) {
        self.stop_timer();

        if self.line_active {
            let _ = write!(self.printer.out_writer(), "\r\x1b[K");
            self.line_active = false;
        }

        self.pending.clear();
    }

    /// Resets all state for a new streaming cycle.
    pub fn reset(&mut self) {
        self.stop_timer();
        self.pending.clear();
        self.rendered.clear();
        self.line_active = false;
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
        let _ = write!(self.printer.out_writer(), "{content}");
    }

    fn rewrite_temp_line(&self) {
        let content = self.temp_line_content();
        let _ = write!(self.printer.out_writer(), "\r{content}\x1b[K");
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
/// This is a free function (not a method) so it can be called from
/// `spawn_blocking` without cloning the entire `ToolRenderer`.
///
/// Arguments with empty values (`{}`, `[]`, `null`) are stripped before
/// formatting.
///
/// - `Off` → `""`
/// - `Json` → JSON block with arguments
/// - `FunctionCall` → `(key=value, ...)`
/// - `Custom(cmd)` → runs the command with tool context, uses stdout verbatim.
///   Returns `Err` if the command fails.
pub(crate) async fn format_args(
    tool_name: &str,
    arguments: &serde_json::Map<String, Value>,
    style: &ParametersStyle,
    root: &Utf8Path,
) -> Result<String, String> {
    let filtered = filter_display_args(arguments);

    if filtered.is_empty() {
        return Ok(String::new());
    }

    match style {
        ParametersStyle::Off => Ok(String::new()),

        ParametersStyle::Json => Ok(format_args_json(&filtered)),

        ParametersStyle::Custom(cmd_config) => {
            format_args_custom(tool_name, &filtered, cmd_config, root).await
        }

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
            Ok(buf)
        }
    }
}

/// Filters out visually empty arguments before display.
fn filter_display_args(
    arguments: &serde_json::Map<String, Value>,
) -> serde_json::Map<String, Value> {
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

fn format_args_json(arguments: &serde_json::Map<String, Value>) -> String {
    let args = serde_json::to_string_pretty(arguments)
        .unwrap_or_else(|_| format!("{:#}", Value::Object(arguments.clone())));
    format!(" with arguments:\n\n```json\n{args}\n```")
}

async fn format_args_custom(
    tool_name: &str,
    arguments: &serde_json::Map<String, Value>,
    cmd_config: &jp_config::conversation::tool::CommandConfigOrString,
    root: &Utf8Path,
) -> Result<String, String> {
    use jp_llm::{CommandResult, run_tool_command};
    use tokio_util::sync::CancellationToken;

    if arguments.is_empty() {
        return Ok(String::new());
    }

    let cmd = cmd_config.clone().command();
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

    let result = run_tool_command(cmd.clone(), ctx, root, CancellationToken::new())
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
        CommandResult::Success(content) => {
            let content = content.trim();
            if content.is_empty() {
                Ok(String::new())
            } else {
                Ok(format!(":\n\n{content}"))
            }
        }
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
        } => {
            let content = stdout.trim();
            if content.is_empty() {
                Ok(String::new())
            } else {
                Ok(format!(":\n\n{content}"))
            }
        }
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
#[path = "renderer_tests.rs"]
mod tests;
