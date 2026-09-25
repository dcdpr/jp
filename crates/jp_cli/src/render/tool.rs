use std::{
    collections::HashMap,
    env, fmt,
    fmt::Write as _,
    fs,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time,
    time::Duration,
};

use crossterm::style::Stylize as _;
use jp_config::{
    conversation::tool::style::{InlineResults, LinkStyle, ParametersStyle, TruncateLines},
    style::{StyleConfig, stderr_rows::StderrRows},
};
use jp_conversation::event::ToolCallResponse;
use jp_md::format::Formatter;
use jp_printer::{ErrChannel, LineSink, OutputLines, RegionStyle, StatusRegion};
use jp_term::{background::DefaultBackground, osc::hyperlink, shade::ShadedWriter};
use serde_json::{Map, Value};

/// Map the `stderr_rows` config key onto the printer's window budget.
///
/// The two enums are deliberately separate: `jp_printer` knows nothing about
/// JP's config tree, and the config key is a user-facing contract that outlives
/// any one renderer.
pub(crate) const fn output_lines(stderr_rows: StderrRows) -> OutputLines {
    match stderr_rows {
        StderrRows::Off => OutputLines::Off,
        StderrRows::Auto => OutputLines::Auto,
        StderrRows::Fixed(count) => OutputLines::Rows(count.rows),
    }
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
/// 1. **Streaming phase** — a status region names the tools whose arguments
///    are still arriving, ticking its own elapsed time.
///    Once arguments are complete, a permanent header replaces it.
///    Methods: [`register`], [`complete`].
///
/// 2. **Permission/execution phase** — arguments (if not already rendered),
///    Custom formatter output, progress, and results.
///    Methods: [`render_tool_call`], [`render_approved`], [`start_progress`],
///    [`render_result`].
///
/// [`complete`]: Self::complete
/// [`register`]: Self::register
/// [`render_approved`]: Self::render_approved
/// [`render_result`]: Self::render_result
/// [`render_tool_call`]: Self::render_tool_call
/// [`start_progress`]: Self::start_progress
pub struct ToolRenderer {
    channel: ErrChannel,
    config: StyleConfig,

    /// Markdown formatter used for syntax highlighting code blocks in tool
    /// results.
    formatter: Formatter,

    /// Tools not yet permanently displayed, in registration order.
    pending: Vec<PendingTool>,

    /// The row naming the tools whose arguments are still streaming.
    ///
    /// Held from the first registration until the last tool completes or the
    /// cycle resets; the printer draws it, ticks its elapsed time, and erases
    /// it around every persistent write.
    preparing: StatusRegion,

    /// The elapsed-time row for tools that are taking a while to run.
    progress: StatusRegion,

    /// Whether the last rendered tool output (custom arguments or a result)
    /// owes a blank-line separator before the next tool call header.
    ///
    /// Shared with the [`TurnView`] so visible assistant content can cancel the
    /// debt: when text or reasoning renders after a tool result, it supplies
    /// its own spacing and the next tool header must not add a second blank
    /// line.
    ///
    /// [`TurnView`]: super::TurnView
    separator: Arc<AtomicBool>,

    /// Whether any tool chrome reached the screen since the chat renderer last
    /// entered a tool-call region from other content.
    ///
    /// Shared with the [`TurnView`]: a tool call settled before it drew
    /// anything, such as one whose formatter failed, leaves nothing on screen
    /// for the content after it to be spaced from.
    ///
    /// [`TurnView`]: super::TurnView
    drawn: Arc<AtomicBool>,

    /// Reasoning-region background captured per tool-call ID.
    ///
    /// Populated at the tool-call boundary (via [`set_region`]) when a tool
    /// continues a reasoning region; the tool's permanent header and result are
    /// shaded with it.
    /// Empty in replay and when no region is active.
    ///
    /// [`set_region`]: Self::set_region
    regions: HashMap<String, DefaultBackground>,

    /// The region background for the chrome being written right now.
    ///
    /// Tracks the most recently entered tool-call region, which the live
    /// aggregate temp/progress line uses; [`complete`] realigns it to a tool's
    /// captured region just before that tool's permanent header is rendered.
    ///
    /// [`complete`]: Self::complete
    current_region: Option<DefaultBackground>,
}

impl ToolRenderer {
    pub fn new(channel: ErrChannel, config: StyleConfig) -> Self {
        let formatter = Formatter::new().theme(if channel.pretty_printing_enabled() {
            config.markdown.theme.as_deref()
        } else {
            None
        });

        Self {
            channel,
            config,
            formatter,
            pending: Vec::new(),
            preparing: StatusRegion::inert(),
            progress: StatusRegion::inert(),
            separator: Arc::new(AtomicBool::new(false)),
            drawn: Arc::new(AtomicBool::new(false)),
            regions: HashMap::new(),
            current_region: None,
        }
    }

    /// Handle to the shared owed-separator flag, for wiring to the
    /// [`TurnView`].
    ///
    /// [`TurnView`]: super::TurnView
    pub(crate) fn separator_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.separator)
    }

    /// Handle to the shared drawn-chrome flag, for wiring to the [`TurnView`].
    ///
    /// [`TurnView`]: super::TurnView
    pub(crate) fn drawn_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.drawn)
    }

    /// Record that a tool call put something on screen this renderer did not
    /// write itself, such as a prompt asking one of its questions.
    pub(crate) fn mark_drawn(&self) {
        self.drawn.store(true, Ordering::Relaxed);
    }

    /// Record the reasoning-region background captured for a tool call at the
    /// tool-call boundary.
    ///
    /// `region` is `Some` when the tool continues a reasoning region (the live
    /// boundary returned a background), `None` otherwise.
    /// The value keys the tool's permanent header and result shading by `id`,
    /// and also becomes the currently-active region for the live temp/progress
    /// line.
    pub(crate) fn set_region(&mut self, id: &str, region: Option<DefaultBackground>) {
        match &region {
            Some(bg) => {
                self.regions.insert(id.to_owned(), bg.clone());
            }
            None => {
                self.regions.remove(id);
            }
        }
        self.current_region = region;
    }

    /// Run `write` against the chrome channel, shading its output with `region`
    /// when one is active.
    ///
    /// With `region` set, the writes flow through a [`ShadedWriter`] so the
    /// chrome carries the reasoning-region background to the right edge,
    /// preserving any background the content sets itself; with `None` they go
    /// straight to the channel unchanged.
    /// Write errors on the chrome channel are swallowed, matching the
    /// renderer's other best-effort writes.
    ///
    /// `write` renders into a buffer that reaches the channel as one print
    /// task.
    /// A `write!` with an interpolated argument is several `write_str` calls,
    /// and `ShadedWriter` splits a line further still into background, text,
    /// fill and reset — each of which would otherwise be its own task for the
    /// printer to erase a status region around, mid-line.
    fn write_chrome<F>(&self, region: Option<&DefaultBackground>, write: F)
    where
        F: FnOnce(&mut dyn fmt::Write) -> fmt::Result,
    {
        let mut buffer = String::new();
        if let Some(bg) = region {
            let mut shaded = ShadedWriter::new(&mut buffer, bg);
            let _ = write(&mut shaded);
            let _ = shaded.finish();
        } else {
            let _ = write(&mut buffer);
        }

        if !buffer.is_empty() {
            self.drawn.store(true, Ordering::Relaxed);
        }
        let _ = self.channel.writer().write_str(&buffer);
    }

    /// The reasoning-region background this tool call sits in, if any.
    ///
    /// Chrome the renderer writes is shaded by [`Self::write_chrome`]; a prompt
    /// is written by someone else and has to be handed the background to shade
    /// itself with.
    pub(crate) fn current_region(&self) -> Option<DefaultBackground> {
        self.current_region.clone()
    }

    /// Emit the blank-line separator owed by a preceding tool result or custom
    /// argument block, if any, then clear the debt.
    ///
    /// Writes to `w` so callers can route the separator through the same shaded
    /// burst as the chrome that follows it.
    fn emit_separator_to(&self, w: &mut dyn fmt::Write) -> fmt::Result {
        if self.separator.swap(false, Ordering::Relaxed) {
            writeln!(w)?;
        }
        Ok(())
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

        self.write_chrome(self.current_region.as_ref(), |w| {
            self.emit_separator_to(w)?;
            writeln!(w, "Calling tool {styled_name}{args}")
        });
    }

    /// Renders an approved tool call, printing header and arguments atomically.
    ///
    /// Prints the header with inline-formatted arguments in a single write.
    /// The built-in styles print their arguments inline rather than producing
    /// content a caller persists.
    ///
    /// A `Custom` style is rendered by [`render_custom_result`] instead, from
    /// output the execution service produced.
    ///
    /// [`render_custom_result`]: Self::render_custom_result
    pub fn render_approved(
        &self,
        name: &str,
        arguments: &Map<String, Value>,
        style: &ParametersStyle,
    ) {
        self.render_tool_call(name, arguments, style);
    }

    /// Render custom arguments already formatted by the execution service.
    ///
    /// Prints the "Calling tool X" header followed by the formatted output, and
    /// returns that output for the caller to persist for replay.
    /// An empty description prints only the header, and returns `None`.
    pub(crate) fn render_custom_result(&self, name: &str, content: String) -> Option<String> {
        let styled_name = name.yellow().bold();
        self.write_chrome(self.current_region.as_ref(), |w| {
            self.emit_separator_to(w)?;
            writeln!(w, "Calling tool {styled_name}")
        });
        if content.is_empty() {
            return None;
        }
        self.render_formatted_arguments(&content);
        Some(content)
    }

    /// Render already-formatted custom argument content.
    ///
    /// Used by [`render_approved`] internally and by the replay path when the
    /// stored event has rendered arguments in its metadata.
    ///
    /// [`render_approved`]: Self::render_approved
    pub fn render_formatted_arguments(&self, content: &str) {
        let trimmed = content.trim();
        self.write_chrome(self.current_region.as_ref(), |w| writeln!(w, "\n{trimmed}"));
        self.separator.store(true, Ordering::Relaxed);
    }

    /// Claim the elapsed-time row for tools that are now executing.
    ///
    /// The row ticks itself and is erased around every persistent write until
    /// [`clear_progress`] drops it; claiming twice without releasing replaces
    /// the first claim.
    ///
    /// [`clear_progress`]: Self::clear_progress
    pub fn start_progress(&mut self) {
        let config = &self.config.tool_call.progress;
        self.progress = if config.show {
            self.channel.status_region(
                RegionStyle::new(
                    Duration::from_secs(u64::from(config.delay_secs)),
                    Duration::from_millis(u64::from(config.interval_ms)),
                    |secs, _| format!("⏱ Running… {secs:.1}s"),
                )
                .with_output(output_lines(config.stderr_rows)),
            )
        } else {
            StatusRegion::inert()
        };

        Self::apply_background(&self.progress, self.current_region.as_ref());
    }

    /// Drop the elapsed-time row.
    pub fn clear_progress(&mut self) {
        self.progress.release();
    }

    /// A sink feeding one tool's stderr into the progress window.
    ///
    /// Labelled with the tool's name, so two running in parallel stay apart.
    /// `None` when there is no window to feed, so a tool that floods costs
    /// nothing when nobody asked to watch it.
    ///
    /// This answers only whether a window exists; whether a particular tool
    /// belongs in it is the caller's, from
    /// `conversation.tools.<name>.style.print_stderr`.
    pub fn progress_source(&self, tool: &str) -> Option<LineSink> {
        self.config
            .tool_call
            .progress
            .stderr_rows
            .is_enabled()
            .then(|| self.progress.source(tool))
    }

    /// Renders a tool call result with language detection, truncation, and file
    /// links.
    ///
    /// This method handles the full result rendering flow:
    ///
    /// 1. Parses content to detect if it's JSON and pretty-prints it
    /// 2. Detects language from code fences or content inspection (XML/JSON)
    /// 3. Writes the full content to a temp file for linking
    /// 4. Truncates the displayed output based on `inline_results` config
    /// 5. Renders file links based on `results_file_link` config
    ///
    /// # Arguments
    ///
    /// - `response` - The tool call response containing the result
    /// - `inline_results` - How to display inline results (Off, Full, Truncate)
    /// - `results_file_link` - How to display file links (Off, Full, Osc8)
    #[expect(clippy::too_many_lines)]
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

        // This tool's captured reasoning-region background, if it continued one.
        // The whole result (inline body and file links, OSC 8 hyperlinks
        // included) is shaded with it.
        let region = self.regions.get(&response.id);

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
            if trimmed.starts_with('<')
                && (has_xml_envelope(trimmed) || quick_xml::de::from_str::<Value>(trimmed).is_ok())
            {
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
        let wrote_inline = !matches!(inline_results, InlineResults::Off) && max_lines > 0;
        if wrote_inline {
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
                    let rendered = self.formatter.render_code_line(&with_nl, state, None, 0);
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

            if !output.ends_with('\n') {
                output.push('\n');
            }

            self.write_chrome(region, |w| write!(w, "{output}"));
        }

        // Render file links
        let wrote_link = match results_file_link {
            LinkStyle::Off => false,
            LinkStyle::Full => {
                self.write_chrome(region, |w| writeln!(w, "see: {}", path.display()));
                true
            }
            LinkStyle::Osc8 => {
                self.write_chrome(region, |w| {
                    writeln!(
                        w,
                        "[{}] [{}]",
                        hyperlink(
                            format!("file://{}", path.display()),
                            "open in editor".red().to_string()
                        ),
                        hyperlink(
                            format!("copy://{}", path.display()),
                            "copy to clipboard".red().to_string()
                        )
                    )
                });
                true
            }
        };

        // A rendered result owes a blank-line separator before the next tool
        // call header, but only when it actually produced visible output: an
        // empty result with no file link writes nothing, so it must not push a
        // stray blank line ahead of the next header. The debt is emitted lazily
        // so it survives the streaming temp line, and is dropped by visible
        // assistant content that follows.
        if wrote_inline || wrote_link {
            self.separator.store(true, Ordering::Relaxed);
        }
    }

    /// Registers a new tool call (name known, arguments pending).
    ///
    /// The first registration claims the preparing row; later ones retitle it.
    pub fn register(&mut self, id: &str, name: &str) {
        if self.pending.iter().any(|t| t.id == id) {
            return;
        }

        let first = self.pending.is_empty();
        self.pending.push(PendingTool {
            id: id.to_owned(),
            name: name.to_owned(),
        });

        if first {
            self.preparing = self.claim_preparing();
            Self::apply_background(&self.preparing, self.current_region.as_ref());
        } else {
            self.refresh_preparing();
        }
    }

    /// End a call's pending-arguments display, including an abandoned call.
    ///
    /// This only handles the rewritable temp-line display.
    /// The permanent "Calling tool ..." header is printed later by
    /// [`render_approved`] after the permission decision.
    ///
    /// [`render_approved`]: Self::render_approved
    pub fn complete(&mut self, id: &str) {
        self.pending.retain(|t| t.id != id);

        // A prepared call's permanent header uses its captured region, which
        // can differ from the other calls still on the preparing row.
        self.current_region = self.regions.get(id).cloned();

        if self.pending.is_empty() {
            self.preparing.release();
        } else {
            self.refresh_preparing();
        }
    }

    /// Shade what is written next with the region captured for call `id`.
    ///
    /// A call's header, description, and prompts can be written long after
    /// other calls started streaming, so the region is set from the call's own
    /// capture each time rather than left at whichever call came last.
    pub(crate) fn focus(&mut self, id: &str) {
        self.current_region = self.regions.get(id).cloned();
    }

    /// Returns `true` if there are tools waiting for arguments.
    #[allow(dead_code)]
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Drops the preparing row and every tool waiting on it.
    pub fn cancel_all(&mut self) {
        self.preparing.release();
        self.pending.clear();
    }

    /// Resets all state for a new streaming cycle.
    pub fn reset(&mut self) {
        self.cancel_all();
    }

    /// Claim the preparing row, or an inert handle when it is switched off.
    fn claim_preparing(&self) -> StatusRegion {
        let config = &self.config.tool_call.preparing;
        if !config.show {
            return StatusRegion::inert();
        }

        self.channel.status_region(
            RegionStyle::new(
                Duration::from_secs(u64::from(config.delay_secs)),
                Duration::from_millis(u64::from(config.interval_ms)),
                |secs, detail| {
                    let names = detail.unwrap_or("tools");
                    format!("{names} (receiving arguments… {secs:.1}s)")
                },
            )
            // A zero-delay row paints on claim, before `set_detail` lands; the
            // names have to be there or the first frame reads "tools".
            .with_detail(self.temp_line_content()),
        )
    }

    /// Retitle the preparing row and re-assert the reasoning background it is
    /// drawn against.
    ///
    /// The row is a live aggregate over the tools pending right now, so it
    /// follows whichever reasoning region is active rather than the one it was
    /// claimed under.
    fn refresh_preparing(&self) {
        self.preparing.set_detail(self.temp_line_content());
        Self::apply_background(&self.preparing, self.current_region.as_ref());
    }

    /// Point a region's row background at `region`, or clear it.
    fn apply_background(status: &StatusRegion, region: Option<&DefaultBackground>) {
        match region {
            Some(background) => status.background().set(&background.param),
            None => status.background().clear(),
        }
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

/// Returns `true` when the content is wrapped in a matching pair of root tags
/// (e.g. `<git_blame>...</git_blame>`).
///
/// Tool results commonly use an XML-like envelope whose body is not guaranteed
/// to be well-formed XML: it may embed raw source code or diff content with
/// unescaped `&` or `<`.
/// Detecting the envelope instead of parsing the full document keeps language
/// detection independent of the embedded content.
///
/// The opening tag must be terminated with `>` before any nested tag starts:
/// either immediately after the root name, or — when the name is followed by
/// whitespace and attributes — before the next `<`.
fn has_xml_envelope(content: &str) -> bool {
    let Some(rest) = content.strip_prefix('<') else {
        return false;
    };

    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':')))
        .unwrap_or(rest.len());
    let name = &rest[..end];

    let after = &rest[end..];
    let has_open_tag_end = match after.chars().next() {
        Some('>') => true,
        Some(c) if c.is_ascii_whitespace() => {
            // Attributes allowed, but the opening tag must be terminated
            // before any other tag (`<`) starts.
            matches!(
                after.find(['<', '>']).map(|i| after.as_bytes()[i]),
                Some(b'>')
            )
        }
        _ => false,
    };

    !name.is_empty() && has_open_tag_end && content.ends_with(&format!("</{name}>"))
}

/// Render a JSON representation of the arguments.
fn format_args_json(arguments: Map<String, Value>) -> String {
    let pretty = format!("{:#}", Value::Object(arguments));
    format!(" with arguments:\n\n```json\n{pretty}\n```")
}

#[cfg(test)]
#[path = "tool_tests.rs"]
mod tests;
