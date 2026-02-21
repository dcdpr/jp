//! A markdown buffer that produces valid blocks of markdown from chunks of
//! text.

use std::fmt;

pub use state::FenceType;
use state::{HtmlBlockRule, HtmlType1Tag, HtmlType6Tag, State};

mod state;

/// Type 2 start tag.
const TYPE2_START_TAG: &str = "<!--";

/// Type 3 start tag.
const TYPE3_START_TAG: &str = "<?";

/// Type 4 start tag.
const TYPE4_START_TAG: &str = "<!";

/// Type 5 start tag.
const TYPE5_START_TAG: &str = "<![CDATA[";

/// An event yielded by the buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A complete block of markdown (e.g., a paragraph, a header, a list item).
    Block(String),

    /// The start of a fenced code block.
    FencedCodeStart {
        /// The language of the code block.
        language: String,

        /// The type of fence used (backtick or tilde).
        fence_type: FenceType,

        /// The length of the fence.
        fence_length: usize,
    },

    /// A line of content within a fenced code block (including the newline).
    FencedCodeLine(String),

    /// The end of a fenced code block.
    FencedCodeEnd,

    /// Raw content flushed from the buffer at end-of-stream.
    /// The content may be a partial block if the stream ended mid-parse.
    Flush(String),
}

impl fmt::Display for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FencedCodeStart {
                language,
                fence_type,
                fence_length,
            } => {
                let fence = match fence_type {
                    FenceType::Backtick => "`".repeat(*fence_length),
                    FenceType::Tilde => "~".repeat(*fence_length),
                };

                write!(f, "{fence}{language}")
            }
            Self::FencedCodeEnd => write!(f, "```"),
            Self::Block(s) | Self::FencedCodeLine(s) | Self::Flush(s) => write!(f, "{s}"),
        }
    }
}

/// Holds the internal buffer and the current parsing state.
#[derive(Debug)]
pub struct Buffer {
    /// The internal buffer.
    buffer: String,

    /// The current state.
    state: State,
}

impl Buffer {
    /// Creates a new [`Buffer`].
    #[must_use]
    pub const fn new() -> Self {
        Self {
            buffer: String::new(),
            state: State::AtBoundary,
        }
    }

    /// Appends a chunk of text to the buffer.
    pub fn push(&mut self, chunk: &str) {
        self.buffer.push_str(chunk);
    }

    /// Called at the end of the stream to flush any remaining content.
    #[must_use]
    pub fn flush(&mut self) -> Option<String> {
        if self.buffer.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut self.buffer))
        }
    }

    /// Handles the `AtBoundary` state: we are at a block boundary. We inspect
    /// the start of the buffer to decide what block we're in.
    fn handle_at_boundary(&mut self) -> (Option<Event>, State) {
        // Trim leading blank lines, as they are just block separators.
        let trimmed_buffer = self.buffer.trim_start_matches('\n');

        // If buffer contains only blank lines, wait for more data
        if trimmed_buffer.is_empty() {
            return (None, State::AtBoundary);
        }

        // Trim the blank lines before the content
        let trimmed_len = self.buffer.len() - trimmed_buffer.len();
        if trimmed_len > 0 {
            self.buffer.drain(..trimmed_len);
        }

        if self.buffer.is_empty() {
            return (None, State::AtBoundary);
        }

        // We need at least one full line to decide what block it is.
        let Some(first_line_end) = self.buffer.find('\n') else {
            return (None, State::AtBoundary);
        };

        let first_line = &self.buffer[..first_line_end];
        let (indent_len, line_content) = get_indent(first_line);

        // Check for "Leaf Blocks" that terminate on one line. Per spec, these
        // blocks may be preceded by up to 3 spaces of indentation

        if indent_len <= 3 && is_atx_header(line_content) {
            let block = self.buffer.drain(..=first_line_end).collect();
            return (Some(Event::Block(block)), State::AtBoundary); // Stay at boundary
        }

        if indent_len <= 3 && is_thematic_break(line_content) {
            let block = self.buffer.drain(..=first_line_end).collect();
            return (Some(Event::Block(block)), State::AtBoundary); // Stay at boundary
        }

        if indent_len <= 3 && is_link_ref_def(line_content) {
            let block = self.buffer.drain(..=first_line_end).collect();
            return (Some(Event::Block(block)), State::AtBoundary); // Stay at boundary
        }

        // Check for "Container Blocks" that change our state. Per spec, these
        // blocks may also be preceded by up to 3 spaces of indentation
        if indent_len <= 3
            && let Some((fence_type, fence_length, language)) = is_fenced_code_start(line_content)
        {
            // Drain the opening line
            let _drained = self.buffer.drain(..=first_line_end);
            return (
                Some(Event::FencedCodeStart {
                    language,
                    fence_type,
                    fence_length,
                }),
                State::InFencedCode {
                    fence_type,
                    fence_length,
                    indent: indent_len,
                },
            );
        }

        if indent_len <= 3
            && let Some(rule) = get_html_block_rule(line_content)
        {
            // Change state, return no block.
            return (None, State::InHtmlBlock { block_type: rule });
        }

        if indent_len >= 4 && !line_content.is_empty() {
            return (None, State::InIndentedCode);
        }

        // Default Case: Paragraph-like Block. If it's none of the above, it's a
        // paragraph, list, blockquote, etc. Change state, return no block.
        (None, State::BufferingParagraph)
    }

    /// Handles `BufferingParagraph`: we're in a paragraph-like block. We need
    /// to find its terminator.
    fn handle_buffering_paragraph(&mut self) -> (Option<Event>, State) {
        let mut terminator_pos: Option<usize> = None;
        let mut flush_len: usize = 0;

        // Iterate over all newlines in the buffer to find a terminator.
        for (idx, _) in self.buffer.match_indices('\n') {
            let line_after_start = idx + 1;

            // Check for blank line (e.g., "...\n\n")
            if self
                .buffer
                .get(line_after_start..)
                .is_some_and(|s| s.starts_with('\n'))
            {
                terminator_pos = Some(idx);
                flush_len = line_after_start + 1;
                break;
            }

            // Not a blank line. Get the content of the next line.
            let rest = &self.buffer[line_after_start..];
            if rest.is_empty() {
                continue; // This was the last \n, need more data
            }

            let (next_line_indent, next_line_content) =
                get_indent(rest.lines().next().unwrap_or(""));

            // Check Setext headers first, takes precedence in paragraph
            // context.
            if is_setext_underline(next_line_content) {
                // We found a setext underline. We must include it. Find the end
                // of *this* underline line.
                if let Some(setext_end_pos) = rest.find('\n') {
                    terminator_pos = Some(idx);
                    flush_len = line_after_start + setext_end_pos + 1;
                    break;
                }

                // We found a setext line but not its end. Need more data.
                return (None, State::BufferingParagraph);
            }

            // Interruption by a new block (HTML blocks and < 4 indent code
            // blocks can interrupt)
            if next_line_indent < 4 && is_block_starter(next_line_content) {
                terminator_pos = Some(idx);
                flush_len = line_after_start;
                break;
            }

            // Otherwise, this is just another line of the paragraph. Continue
            // searching.
        }

        if terminator_pos.is_some() {
            let block = self.buffer.drain(..flush_len).collect();
            (Some(Event::Block(block)), State::AtBoundary)
        } else {
            (None, State::BufferingParagraph)
        }
    }

    /// Handles `InIndentedCode`: we're looking for the end of an indented code
    /// block.
    fn handle_in_indented_code(&mut self) -> (Option<Event>, State) {
        let mut scan_pos = 0;
        let mut block_len = 0;
        let mut terminated = false;

        while scan_pos < self.buffer.len() {
            let slice = &self.buffer[scan_pos..];
            let (line, line_len, had_newline) =
                slice.find('\n').map_or((slice, slice.len(), false), |pos| {
                    (&slice[..pos], pos + 1, true)
                });

            let (indent, content) = get_indent(line);

            if indent >= 4 {
                if !had_newline {
                    // Partial indented line: wait for the newline before
                    // flushing.
                    block_len = scan_pos + line_len;
                    break;
                }

                scan_pos += line_len;
                block_len = scan_pos;
                continue;
            }

            if content.is_empty() {
                if !had_newline {
                    // Need more data to decide if this blank line terminates
                    // the block.
                    break;
                }

                match self.indented_code_blank_followed_by_indented(scan_pos + line_len) {
                    Some(true) => {
                        // Include this blank line in the block and keep
                        // scanning.
                        scan_pos += line_len;
                        block_len = scan_pos;
                        continue;
                    }
                    Some(false) => {
                        terminated = true;
                        break;
                    }
                    None => {
                        // Await more data before deciding.
                        break;
                    }
                }
            }

            // Non-blank line with indent < 4 terminates the block.
            terminated = true;
            break;
        }

        if block_len > 0 && terminated {
            let block = self.buffer.drain(..block_len).collect();
            return (Some(Event::Block(block)), State::AtBoundary);
        }

        (None, State::InIndentedCode)
    }

    /// Returns `Some(true)` if the blank line at `start_idx` is followed by an
    /// indented code line, `Some(false)` if it is followed by less-indented
    /// content, and `None` if more data is needed.
    fn indented_code_blank_followed_by_indented(&self, start_idx: usize) -> Option<bool> {
        let mut idx = start_idx;

        while idx < self.buffer.len() {
            let slice = &self.buffer[idx..];
            let (line, line_len, had_newline) =
                slice.find('\n').map_or((slice, slice.len(), false), |pos| {
                    (&slice[..pos], pos + 1, true)
                });

            let (indent, content) = get_indent(line);

            if content.is_empty() {
                if !had_newline {
                    return None;
                }
                idx += line_len;
                continue;
            }

            return Some(indent >= 4);
        }

        None
    }

    /// Handles `InFencedCode`: we process one line at a time.
    fn handle_in_fenced_code(
        &mut self,
        fence_type: FenceType,
        fence_length: usize,
        indent: usize,
    ) -> (Option<Event>, State) {
        let current_state = State::InFencedCode {
            fence_type,
            fence_length,
            indent,
        };

        // We need at least one newline to have a full line
        let Some(line_end) = self.buffer.find('\n') else {
            return (None, current_state);
        };

        let line_content_slice = &self.buffer[..line_end]; // without newline for inspection
        let (indent_len, content) = get_indent(line_content_slice);

        let expected_char = fence_type.as_char();

        // Check for closing fence:
        // 1. Less than 4 spaces of indent.
        // 2. Starts with the correct fence character.
        if indent_len < 4 && content.starts_with(expected_char) {
            // 3. Is at least as long as the opening fence.
            let closing_fence_len = content.chars().take_while(|&c| c == expected_char).count();
            if closing_fence_len >= fence_length {
                // 4. Has no other characters after the fence (except whitespace).
                let after_fence = &content[closing_fence_len..];
                if after_fence.trim().is_empty() {
                    // Found closing fence. Drain line and switch state.
                    let _drained = self.buffer.drain(..=line_end);
                    return (Some(Event::FencedCodeEnd), State::AtBoundary);
                }
            }
        }

        // It is a code line.
        let mut raw_line = self.buffer.drain(..=line_end).collect::<String>();

        // Strip up to `indent` spaces from the beginning of the line
        // if they exist.
        //
        // Note: `raw_line` contains the newline.

        // Count leading spaces
        let leading_spaces = raw_line.chars().take_while(|&c| c == ' ').count();
        let spaces_to_strip = std::cmp::min(leading_spaces, indent);

        if spaces_to_strip > 0 {
            raw_line.drain(..spaces_to_strip);
        }

        (Some(Event::FencedCodeLine(raw_line)), current_state)
    }

    /// Handles `InHtmlBlock`: look for termination based on its rule.
    fn handle_in_html_block(&mut self, block_type: HtmlBlockRule) -> (Option<Event>, State) {
        let current_state = State::InHtmlBlock { block_type };
        let end_pos = self.buffer.find(block_type.end_tag());
        let terminator = match block_type {
            HtmlBlockRule::Type6(_) | HtmlBlockRule::Type7 => end_pos.map(|pos| pos + 2),
            _ => end_pos.and_then(|tag_pos| {
                self.buffer[tag_pos..]
                    .find('\n')
                    .map(|line_end| tag_pos + line_end + 1)
            }),
        };

        terminator
            .map(|pos| self.buffer.drain(..pos).collect())
            .map_or((None, current_state), |block| {
                (Some(Event::Block(block)), State::AtBoundary)
            })
    }
}

impl From<&str> for Buffer {
    fn from(s: &str) -> Self {
        let mut buf = Self::new();
        buf.push(s);
        buf
    }
}

impl Default for Buffer {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Write for Buffer {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.push(s);
        Ok(())
    }
}

impl Iterator for Buffer {
    type Item = Event;

    /// Fetches the next completed markdown block from the internal buffer.
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let current_state = self.state;
            let (maybe_block, next_state) = match self.state {
                State::AtBoundary => self.handle_at_boundary(),
                State::BufferingParagraph => self.handle_buffering_paragraph(),
                State::InIndentedCode => self.handle_in_indented_code(),
                State::InFencedCode {
                    fence_type,
                    fence_length,
                    indent,
                } => self.handle_in_fenced_code(fence_type, fence_length, indent),
                State::InHtmlBlock { block_type } => self.handle_in_html_block(block_type),
            };

            // We found a complete block or event. Update state and return it.
            if let Some(block) = maybe_block {
                self.state = next_state;
                return Some(block);
            }

            // If the state *did not change*, it means the current handler is
            // waiting for more data. We break and return None.
            if next_state == current_state {
                return None;
            }

            self.state = next_state;

            // If the state *did change* (e.g., AtBoundary -> InFencedCode), but
            // we did not get a complete block back, we loop again to
            // immediately process in the new state.
        }
    }
}

/// Calculate the indentation of a line, and return the line content without the
/// indentation.
///
/// Per Commonmark spec §2.2, tabs are treated as advancing to the next column
/// that is a multiple of 4 (tab stop of 4).
///
/// Returns (`effective_indent_in_spaces`, `content_after_indentation`)
fn get_indent(line: &str) -> (usize, &str) {
    let mut column = 0;
    let mut bytes_consumed = 0;

    for ch in line.chars() {
        match ch {
            ' ' => {
                column += 1;
                bytes_consumed += 1;
            }
            '\t' => {
                // Tab advances to next multiple of 4
                let next_tab_stop = (column + 4) / 4 * 4;
                column = next_tab_stop;
                bytes_consumed += 1;
            }
            _ => break, // Found non-whitespace
        }
    }

    (column, &line[bytes_consumed..])
}

/// Checks if a line (without indent) can start a new block (and thus interrupt
/// a paragraph)
fn is_block_starter(line: &str) -> bool {
    is_atx_header(line)
        || is_thematic_break(line)
        || is_fenced_code_start(line).is_some()
        || get_html_block_rule(line).is_some_and(|r| !matches!(r, HtmlBlockRule::Type7))
}

/// Get the rule for an HTML block.
fn get_html_block_rule(line: &str) -> Option<HtmlBlockRule> {
    if !line.starts_with('<') {
        return None;
    }

    // Check for Types 2, 3, 5 first (most specific)
    if line.starts_with(TYPE2_START_TAG) {
        return Some(HtmlBlockRule::Type2);
    }
    if line.starts_with(TYPE3_START_TAG) {
        return Some(HtmlBlockRule::Type3);
    }
    if line.starts_with(TYPE5_START_TAG) {
        return Some(HtmlBlockRule::Type5);
    }

    // Check for Type 4
    if line.starts_with(TYPE4_START_TAG)
        && line.len() > 2
        && line.as_bytes()[2].is_ascii_alphabetic()
    {
        return Some(HtmlBlockRule::Type4);
    }

    // Get tag name for Types 1, 6, 7
    let after_slash = line.strip_prefix("</").unwrap_or_else(|| &line[1..]);
    let tag_name_end = after_slash
        .find(|c: char| c.is_whitespace() || c == '>')
        .unwrap_or(after_slash.len());
    let tag_name = &after_slash[..tag_name_end].to_lowercase();

    if tag_name.is_empty() {
        return None; // e.g. "<>"
    }

    // Check for Type 1
    if let Some(tag) = HtmlType1Tag::from_str(tag_name.as_str()) {
        return Some(HtmlBlockRule::Type1(tag));
    }

    // Check for Type 6
    if let Some(tag) = HtmlType6Tag::from_str(tag_name.as_str()) {
        return Some(HtmlBlockRule::Type6(tag));
    }

    // Check for Type 7
    if line.trim_end().ends_with('>') {
        return Some(HtmlBlockRule::Type7);
    }

    None
}

/// Checks if a line (without indent) is an ATX Header
///
/// Per Commonmark spec §4.2, must have 1-6 `#` followed by space, tab, or EOL
fn is_atx_header(line: &str) -> bool {
    if !line.starts_with('#') {
        return false;
    }

    // Count the number of # characters
    let hash_count = line.chars().take_while(|&c| c == '#').count();

    // Must be between 1 and 6 (spec requires max 6)
    if !(1..=6).contains(&hash_count) {
        return false;
    }

    // Must be followed by space, tab, or end of line
    let Some(first_char) = &line[hash_count..].chars().next() else {
        return true;
    };

    *first_char == ' ' || *first_char == '\t'
}

/// Checks if a line (without indent) is a Thematic Break
fn is_thematic_break(line: &str) -> bool {
    let s = line.trim();
    let Some(first) = s.chars().next() else {
        return false;
    };

    if !(first == '*' || first == '-' || first == '_') {
        return false;
    }
    let mut count = 0;
    for c in s.chars() {
        if c == first {
            count += 1;
        } else if c.is_whitespace() {
        } else {
            return false; // Mixed characters
        }
    }
    count >= 3
}

/// Checks if a line (without indent) is a Fenced Code Start
fn is_fenced_code_start(line: &str) -> Option<(FenceType, usize, String)> {
    let s = line.trim_end();
    let fence_char = s.chars().next()?;

    let fence_type = match fence_char {
        '`' => FenceType::Backtick,
        '~' => FenceType::Tilde,
        _ => return None,
    };

    let fence_len = s.chars().take_while(|&c| c == fence_char).count();
    if fence_len < 3 {
        return None;
    }

    let info_string = &s[fence_len..].trim();

    // Info string for backticks cannot contain backticks
    if fence_type == FenceType::Backtick && info_string.contains('`') {
        return None;
    }

    Some((fence_type, fence_len, info_string.to_string()))
}

/// Checks if a line (without indent) is a Link Reference Definition
fn is_link_ref_def(line: &str) -> bool {
    // This is a simplified check. A full one can be added later. We just check
    // for the `[label]: url` structure.
    if !line.starts_with('[') {
        return false;
    }
    line.find("]:")
        .is_some_and(|label_end| !line[label_end + 2..].trim_start().is_empty())
}

/// Checks if a line (without indent) is a Setext Underline
fn is_setext_underline(line: &str) -> bool {
    let s = line.trim();
    if s.is_empty() {
        return false;
    }
    s.chars().all(|c| c == '=') || s.chars().all(|c| c == '-')
}

#[cfg(test)]
#[path = "buffer_tests.rs"]
mod tests;
