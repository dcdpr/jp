use std::{fs, path::PathBuf, time::Duration};

use crossterm::style::{Color, Stylize as _};
use jp_config::style::{LinkStyle, StyleConfig};
use jp_printer::{PrintableExt as _, Printer};
use jp_term::{code, osc::hyperlink, stdout};
use termimad::FmtText;

use super::{Line, LineVariant, RenderMode};
use crate::Error;

#[derive(Debug)]
pub(super) struct ResponseHandler {
    /// How to render the response.
    pub render_mode: RenderMode,

    /// Whether to render tool call request/results.
    pub render_tool_calls: bool,

    /// The streamed, unprocessed lines received from the LLM.
    received: Vec<String>,

    pub printer: Arc<Printer>,

    /// The lines that have been parsed so far.
    ///
    /// If `should_stream` is `true`, these lines have been printed to the
    /// terminal. Otherwise they will be printed when the response handler is
    /// finished.
    pub parsed: Vec<String>,

    /// A temporary buffer of data received from the LLM.
    pub buffer: String,

    in_fenced_code_block: bool,
    // (language, code)
    code_buffer: (Option<String>, Vec<String>),
    code_line: usize,

    // The last index of the line that ends a code block.
    // (streamed, printed)
    last_fenced_code_block_end: (usize, usize),
}

impl ResponseHandler {
    pub fn new(render_mode: RenderMode, render_tool_calls: bool, printer: Arc<Printer>) -> Self {
        Self {
            render_mode,
            render_tool_calls,
            printer,
            received: vec![],
            parsed: vec![],
            buffer: String::new(),
            in_fenced_code_block: false,
            code_buffer: (None, vec![]),
            code_line: 0,
            last_fenced_code_block_end: (0, 0),
        }
    }

    pub fn drain(&mut self, style: &StyleConfig, raw: bool) -> Result<(), Error> {
        if self.buffer.is_empty() {
            return Ok(());
        }

        let line = Line::new(
            self.buffer.drain(..).collect(),
            self.in_fenced_code_block,
            raw,
        );

        self.handle_inner(line, style)
    }

    fn handle_inner(&mut self, line: Line, style: &StyleConfig) -> Result<(), Error> {
        let Line { content, variant } = line;
        self.received.push(content);

        let delay = match variant {
            LineVariant::Code => style.typewriter.code_delay.into(),
            LineVariant::Raw => Duration::ZERO,
            LineVariant::Normal => style.typewriter.text_delay.into(),
        };

        let lines = self.handle_line(&variant, style)?;
        if !matches!(self.render_mode, RenderMode::Buffered) {
            self.printer.print(lines.join("\n").typewriter(delay));
        }

        self.parsed.extend(lines);

        Ok(())
    }

    #[expect(clippy::too_many_lines)]
    fn handle_line(
        &mut self,
        variant: &LineVariant,
        style: &StyleConfig,
    ) -> Result<Vec<String>, Error> {
        let Some(content) = self.received.last().map(String::as_str) else {
            return Ok(vec![]);
        };

        match variant {
            LineVariant::Raw => Ok(content.lines().map(str::to_owned).collect()),
            LineVariant::Code => {
                self.code_line += 1;
                self.code_buffer.1.push(content.to_owned());

                let mut buf = String::new();
                let config = code::Config {
                    language: self.code_buffer.0.clone(),
                    theme: style.code.color.then(|| style.code.theme.clone()),
                };

                if !code::format(content, &mut buf, &config)? {
                    let config = code::Config {
                        language: None,
                        theme: config.theme,
                    };

                    code::format(content, &mut buf, &config)?;
                }

                if style.code.line_numbers {
                    buf.insert_str(
                        0,
                        &format!("{:2} │ ", self.code_line)
                            .with(Color::AnsiValue(238))
                            .to_string(),
                    );
                }

                Ok(vec![buf])
            }
            LineVariant::FencedCodeBlockStart { language } => {
                self.code_buffer.0.clone_from(language);
                self.code_buffer.1.clear();
                self.code_line = 0;
                self.in_fenced_code_block = true;

                Ok(vec![content.with(Color::AnsiValue(238)).to_string()])
            }
            LineVariant::FencedCodeBlockEnd { indent } => {
                self.last_fenced_code_block_end = (self.received.len(), self.parsed.len() + 2);

                let path = self.persist_code_block()?;
                let mut links = vec![];

                match style.code.file_link {
                    LinkStyle::Off => {}
                    LinkStyle::Full => {
                        links.push(format!("{}see: {}", " ".repeat(*indent), path.display()));
                    }
                    LinkStyle::Osc8 => {
                        links.push(format!(
                            "{}[{}]",
                            " ".repeat(*indent),
                            hyperlink(
                                format!("file://{}", path.display()),
                                "open in editor".red().to_string()
                            )
                        ));
                    }
                }

                match style.code.copy_link {
                    LinkStyle::Off => {}
                    LinkStyle::Full => {
                        links.push(format!(
                            "{}copy: copy://{}",
                            " ".repeat(*indent),
                            path.display()
                        ));
                    }
                    LinkStyle::Osc8 => {
                        links.push(format!(
                            "{}[{}]",
                            " ".repeat(*indent),
                            hyperlink(
                                format!("copy://{}", path.display()),
                                "copy to clipboard".red().to_string()
                            )
                        ));
                    }
                }

                self.in_fenced_code_block = false;

                let mut lines = vec![content.with(Color::AnsiValue(238)).to_string()];
                if !links.is_empty() {
                    lines.push(links.join(" "));
                }

                Ok(lines)
            }
            LineVariant::Normal => {
                // We feed all the lines for markdown formatting, but only
                // print the last one, as the others are already printed.
                //
                // This helps the parser to use previous context to apply
                // the correct formatting to the current line.
                //
                // We only care about the lines after the last code block
                // end, because a) formatting context is reset after a code
                // block, and b) we dot not limit the line length of code, makes
                // it impossible to correctly find the non-printed lines based
                // on wrapped vs non-wrapped lines.
                let lines = self
                    .received
                    .iter()
                    .skip(self.last_fenced_code_block_end.0)
                    .cloned()
                    .collect::<Vec<_>>();

                // `termimad` removes empty lines at the start or end, but we
                // want to keep them as we will have more lines to print.
                let empty_lines_start_count = lines.iter().take_while(|s| s.is_empty()).count();
                let empty_lines_end_count = lines.iter().rev().take_while(|s| s.is_empty()).count();

                let options = comrak::Options {
                    render: comrak::RenderOptions {
                        unsafe_: true,
                        prefer_fenced: true,
                        experimental_minimize_commonmark: true,
                        ..Default::default()
                    },
                    ..Default::default()
                };

                let formatted = comrak::markdown_to_commonmark(&lines.join("\n"), &options);

                let mut formatted =
                    FmtText::from(&termimad::MadSkin::default(), &formatted, Some(100)).to_string();

                for _ in 0..empty_lines_start_count {
                    formatted.insert(0, '\n');
                }

                // Only add an extra newline if we have more than one line,
                // otherwise a single empty line will be interpreted as both a
                // missing start and end newline.
                if lines.iter().any(|s| !s.is_empty()) {
                    for _ in 0..empty_lines_end_count {
                        formatted.push('\n');
                    }
                }

                let lines = formatted
                    .lines()
                    .skip(self.parsed.len() - self.last_fenced_code_block_end.1)
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>();

                Ok(lines)
            }
        }
    }

    fn get_line(&mut self, raw: bool) -> Option<Line> {
        let s = &mut self.buffer;
        let idx = s.find('\n')?;

        // Determine the end index of the actual line *content*.
        // Check if the character before '\n' is '\r'.
        let end_idx = if idx > 0 && s.as_bytes().get(idx - 1) == Some(&b'\r') {
            idx - 1
        } else {
            idx
        };

        // Extract the line content *before* draining.
        // Creating a slice and then converting to owned String.
        let extracted_line = s[..end_idx].to_string();

        // Calculate the index *after* the newline sequence to drain up to.
        // This ensures we remove the '\n' and potentially the preceding '\r'.
        let drain_end_idx = idx + 1;
        s.drain(..drain_end_idx);

        Some(Line::new(extracted_line, self.in_fenced_code_block, raw))
    }

    fn persist_code_block(&self) -> Result<PathBuf, Error> {
        let code = self.code_buffer.1.clone();
        let language = self.code_buffer.0.as_deref().unwrap_or("txt");
        let ext = match language {
            "c++" => "cpp",
            "javascript" => "js",
            "python" => "py",
            "ruby" => "rb",
            "rust" => "rs",
            "typescript" => "ts",
            lang => lang,
        };

        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_millis();
        let path = std::env::temp_dir().join(format!("code_{millis}.{ext}"));

        fs::write(&path, code.join("\n"))?;

        Ok(path)
    }
}
