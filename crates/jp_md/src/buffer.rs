//! A markdown buffer that splits a stream of text chunks into renderable
//! blocks.
//!
//! [`Buffer`] is the entry point: push text with [`Buffer::push`], pull
//! [`Event`]s through the [`Iterator`] impl, and drain the tail with
//! [`Buffer::flush_events`].
//! Each event is one block — a paragraph, header, list item, fenced-code line,
//! and so on — ready for the renderer.
//!
//! # Streaming paragraphs
//!
//! Top-level paragraphs stream incrementally, on by default.
//! Once a paragraph grows past an internal threshold the buffer emits its
//! leading prose as [`Event::ParagraphChunk`]s rather than waiting for the
//! terminator, so a long single-line paragraph (common in assistant output)
//! renders as it arrives instead of stalling until it ends.
//! Concatenating a paragraph's chunks yields exactly the [`Event::Block`] that
//! non-streaming buffering emits, so a consumer that re-renders the accumulated
//! source produces byte-identical output.
//!
//! GFM tables do not stream: their column widths depend on later rows, so a
//! table is kept whole, detected by the leading pipe on its header row.
//!
//! [`Buffer::with_streaming_paragraphs`] turns streaming off, restoring
//! whole-paragraph [`Event::Block`]s.
//! Streamed output is byte-identical to that mode except for two edge cases,
//! neither produced by assistant output: an over-threshold setext heading
//! streams as prose instead of becoming a heading, and a table header without a
//! leading pipe (legal per the GFM spec but unexemplified there) is not
//! detected, so it streams and may render with mis-padded columns.

use std::fmt;

pub use fixup::{EventFixup, FenceEscalationFixup, Fixups, OrphanedFenceFixup};
pub use state::FenceType;
use state::{HtmlBlockRule, HtmlTerminator, HtmlType1Tag, HtmlType6Tag, ListState, State};

pub mod fixup;
mod state;

/// Type 2 start tag.
const TYPE2_START_TAG: &str = "<!--";

/// Type 3 start tag.
const TYPE3_START_TAG: &str = "<?";

/// Type 4 start tag.
const TYPE4_START_TAG: &str = "<!";

/// Type 5 start tag.
const TYPE5_START_TAG: &str = "<![CDATA[";

/// Minimum buffered source bytes before a top-level paragraph begins streaming.
///
/// Below this a paragraph buffers whole, so a short setext underline can still
/// turn it into a heading; above it the paragraph is too long to be a realistic
/// heading and streams as prose.
const SETEXT_STREAM_THRESHOLD: usize = 128;

/// An event yielded by the buffer.
///
/// Every event carries an `indent` field giving the visual column at which the
/// consumer should render the event's content.
/// This is used by the streaming buffer to emit events from inside nested
/// containers (list items, fenced code inside list items) at their correct
/// visual indent.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Event {
    /// A complete block of markdown (e.g., a paragraph, a header, a list item).
    Block {
        /// The block's markdown content.
        content: String,
        /// Visual indent (in spaces) the renderer should apply.
        indent: usize,
    },

    /// A streamed slice of a top-level paragraph's source.
    ///
    /// Emitted, instead of a single [`Block`], while a top-level paragraph is
    /// buffered past the streaming threshold, so its leading text can render
    /// before the paragraph terminates.
    /// The consumer accumulates each chunk's `content` and re-renders the
    /// growing paragraph; concatenating every chunk of one paragraph yields
    /// exactly the `Block` content that non-streaming buffering would emit.
    ///
    /// [`Block`]: Self::Block
    ParagraphChunk {
        /// New paragraph source since the previous chunk (a delta, never
        /// cumulative).
        /// A non-terminal chunk (`last == false`) never ends inside an open
        /// inline construct; the terminal chunk (`last == true`) carries
        /// whatever source remains at the region boundary and may.
        content: String,
        /// Visual indent (in spaces) the renderer should apply.
        /// Always `0`: chunks come only from top-level paragraphs.
        indent: usize,
        /// Whether this chunk closes the paragraph for rendering — either a
        /// real terminator was seen or the content region ended.
        last: bool,
    },

    /// The start of a fenced code block.
    FencedCodeStart {
        /// The language of the code block.
        language: String,

        /// The type of fence used (backtick or tilde).
        fence_type: FenceType,

        /// The length of the fence.
        fence_length: usize,

        /// Visual indent (in spaces) the renderer should apply.
        indent: usize,
    },

    /// A line of content within a fenced code block (including the newline).
    FencedCodeLine {
        /// The code line, with the fence's own indent already stripped.
        content: String,
        /// Visual indent (in spaces) the renderer should apply.
        indent: usize,
    },

    /// The end of a fenced code block.
    FencedCodeEnd {
        /// The closing fence string (e.g.
        /// ` ``` ` or `~~~~~`).
        fence: String,
        /// Visual indent (in spaces) the renderer should apply.
        indent: usize,
    },

    /// Raw content flushed from the buffer at end-of-stream.
    /// The content may be a partial block if the stream ended mid-parse.
    Flush {
        /// The remaining buffer content.
        content: String,
        /// Visual indent (in spaces) the renderer should apply.
        indent: usize,
    },
}

impl Event {
    /// Construct a [`Event::Block`] with no indent.
    #[must_use]
    pub fn block(content: impl Into<String>) -> Self {
        Self::Block {
            content: content.into(),
            indent: 0,
        }
    }

    /// Construct a [`Event::FencedCodeLine`] with no indent.
    #[must_use]
    pub fn fenced_code_line(content: impl Into<String>) -> Self {
        Self::FencedCodeLine {
            content: content.into(),
            indent: 0,
        }
    }

    /// Construct a [`Event::FencedCodeEnd`] with no indent.
    #[must_use]
    pub fn fenced_code_end(fence: impl Into<String>) -> Self {
        Self::FencedCodeEnd {
            fence: fence.into(),
            indent: 0,
        }
    }

    /// Construct a [`Event::Flush`] with no indent.
    #[must_use]
    pub fn flush(content: impl Into<String>) -> Self {
        Self::Flush {
            content: content.into(),
            indent: 0,
        }
    }

    /// Construct a [`Event::ParagraphChunk`] with no indent.
    #[must_use]
    pub fn paragraph_chunk(content: impl Into<String>, last: bool) -> Self {
        Self::ParagraphChunk {
            content: content.into(),
            indent: 0,
            last,
        }
    }
}

impl fmt::Display for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FencedCodeStart {
                language,
                fence_type,
                fence_length,
                ..
            } => {
                let fence = match fence_type {
                    FenceType::Backtick => "`".repeat(*fence_length),
                    FenceType::Tilde => "~".repeat(*fence_length),
                };

                write!(f, "{fence}{language}")
            }
            Self::Block { content: s, .. }
            | Self::FencedCodeLine { content: s, .. }
            | Self::FencedCodeEnd { fence: s, .. }
            | Self::Flush { content: s, .. }
            | Self::ParagraphChunk { content: s, .. } => {
                write!(f, "{s}")
            }
        }
    }
}

/// Holds the internal buffer and the current parsing state.
#[derive(Debug)]
pub struct Buffer {
    /// The internal buffer.
    data: String,

    /// The current state.
    state: State,

    /// Stack of saved parent states for nested containers.
    ///
    /// When entering a nested context (e.g., a fence inside a list item, or a
    /// list inside a list item), the current state is pushed here and replaced
    /// with the inner state.
    /// When the inner state closes, the parent is popped back as the active
    /// state.
    parents: Vec<State>,

    /// Whether top-level paragraphs stream incrementally as
    /// [`Event::ParagraphChunk`]s rather than buffering to a single
    /// [`Event::Block`].
    streaming: bool,

    /// Bytes of the current top-level paragraph already emitted as
    /// [`Event::ParagraphChunk`]s.
    ///
    /// Indexes into `data`, which is not drained while a paragraph streams.
    /// `0` whenever no paragraph is mid-stream.
    para_emitted: usize,
}

impl Buffer {
    /// Creates a new [`Buffer`].
    #[must_use]
    pub const fn new() -> Self {
        Self {
            data: String::new(),
            state: State::AtBoundary,
            parents: Vec::new(),
            streaming: true,
            para_emitted: 0,
        }
    }

    /// Toggle incremental streaming of top-level paragraphs.
    ///
    /// Streaming is enabled by default.
    /// With it disabled the buffer emits each paragraph as a single
    /// [`Event::Block`] once a terminator is seen, never an
    /// [`Event::ParagraphChunk`].
    ///
    /// Streamed output is byte-identical to whole-paragraph buffering except
    /// for two edge cases, neither produced by assistant output: an
    /// over-threshold setext heading streams as prose instead of becoming a
    /// heading, and a GFM table whose header lacks a leading pipe is not
    /// detected and may stream with mis-padded columns.
    /// Disable streaming to render either exactly.
    #[must_use]
    pub const fn with_streaming_paragraphs(mut self, on: bool) -> Self {
        self.streaming = on;
        self
    }

    /// State to return to when the current state's block closes.
    fn pop_parent_or_boundary(&mut self) -> State {
        self.parents.pop().unwrap_or(State::AtBoundary)
    }

    /// Appends a chunk of text to the buffer.
    pub fn push(&mut self, chunk: &str) {
        self.data.push_str(chunk);
    }

    /// Drain all remaining content and emit it as a sequence of events.
    ///
    /// Called at the end of the stream.
    /// For a `Buffer` that's mid-list with several complete items still queued
    /// (because the buffer can't flush an item until the next line is fully
    /// received), this emits each complete item as its own `Block` event with
    /// the correct renumbered marker and visual indent.
    /// The trailing partial segment becomes the final `Flush` event.
    ///
    /// For non-list states the entire remainder is emitted as a single `Flush`
    /// event with the active indent (stripping the fence's indent for
    /// `InFencedCode`, leaving content as-is otherwise).
    pub fn flush_events(&mut self) -> Vec<Event> {
        // An open fenced code block is handled specially: end-of-region
        // completes the final line, so a closing fence that arrived without a
        // trailing newline is still recognized, and a block that never closed
        // is balanced with a synthetic closing fence. This drains the fence
        // and every now-complete trailing block through the state machine,
        // which can transition us out of `InFencedCode` with one trailing
        // partial block still buffered (e.g. a paragraph after the close).
        let mut events = if matches!(self.state, State::InFencedCode { .. }) {
            self.flush_fenced_code_events()
        } else {
            Vec::new()
        };

        // Flush whatever remains as a trailing partial, in the current
        // (possibly just-transitioned) state. `flush_fenced_code_events`
        // emits every *complete* block via the iterator, so at most one
        // partial block is left here; the non-fenced path takes this branch
        // directly with its untouched buffer.
        let raw = std::mem::take(&mut self.data);
        if !raw.is_empty() {
            match self.state {
                State::InList(list) => events.extend(Self::flush_list_events(&raw, list)),
                // A paragraph that began streaming closes its region with a
                // terminal `ParagraphChunk` carrying the remaining source, not
                // a `Flush` (which the consumer would re-render as a fresh
                // standalone block).
                State::BufferingParagraph if self.streaming && self.para_emitted > 0 => {
                    let content = raw[self.para_emitted..].to_string();
                    events.push(Event::paragraph_chunk(content, true));
                }
                _ => events.push(Event::Flush {
                    content: raw,
                    indent: self.current_indent(),
                }),
            }
        }

        // `flush_events` is the explicit "wipe the slate" boundary:
        // `ChatRenderer::flush()` calls this on every content-kind
        // transition (reasoning ↔ message ↔ tool call, role headers,
        // user echos), not only at end-of-stream. Any data pushed
        // afterwards must be parsed in `AtBoundary` rather than as
        // continuation of the just-flushed block, so reset the state
        // and clear the parent stack on every path — including the
        // empty-buffer fast path.
        self.state = State::AtBoundary;
        self.parents.clear();
        self.para_emitted = 0;

        events
    }

    /// Flush an open fenced code block at end-of-region.
    ///
    /// End-of-region completes the final buffered line, so a closing fence that
    /// arrived without a trailing newline is recognized as a
    /// [`Event::FencedCodeEnd`] rather than dumped as opaque text; preceding
    /// partial lines are emitted as [`Event::FencedCodeLine`]s.
    /// If no closing fence is present at all, a synthetic one is emitted so the
    /// opening fence is always balanced for downstream consumers.
    fn flush_fenced_code_events(&mut self) -> Vec<Event> {
        // Treat the end of the region as a line terminator so the final line
        // parses (the in-fence handler only acts on newline-terminated lines).
        if !self.data.is_empty() && !self.data.ends_with('\n') {
            self.data.push('\n');
        }

        // Drain the now-complete lines through the normal state machine.
        let mut events: Vec<Event> = self.by_ref().collect();

        // Still inside the block means no closing fence was present: synthesize
        // one at the source fence length. The escalation fixup, if any, widens
        // it to match the (also-escalated) opening fence.
        if let State::InFencedCode {
            fence_type,
            fence_length,
            indent,
            ..
        } = self.state
        {
            let fence = fence_type.as_char().to_string().repeat(fence_length);
            events.push(Event::FencedCodeEnd { fence, indent });
        }

        events
    }

    /// Implements [`Self::flush_events`] for `InList` state: scan the remaining
    /// buffer for item boundaries (using the same line classifier as the
    /// streaming walk), emit each complete preceding segment as a `Block`
    /// (renumbered against the list's start), and emit the final segment as a
    /// `Flush`.
    fn flush_list_events(raw: &str, list: ListState) -> Vec<Event> {
        // Walk lines, splitting at every boundary line: a sibling marker
        // (next item of this list) or a foreign marker at the marker column
        // (a new list of a different kind). `classify_list_line` is the
        // single source of truth for that distinction, shared with the
        // streaming walk in `handle_in_list`.
        let mut segments: Vec<(usize, usize)> = Vec::new();
        let mut seg_start = 0;
        let mut idx = 0;
        let mut prev_blank = false;

        while idx < raw.len() {
            let line_end = raw[idx..].find('\n').map_or(raw.len(), |p| idx + p);
            let line = &raw[idx..line_end];
            let (indent, content) = get_indent(line);

            if content.is_empty() {
                prev_blank = true;
            } else {
                let kind = classify_list_line(indent, content, list, prev_blank);
                let is_boundary = match kind {
                    ListLineKind::SiblingMarker => true,
                    ListLineKind::Terminator => {
                        indent == list.marker_column && is_list_marker(content)
                    }
                    ListLineKind::NestedContainer | ListLineKind::Continuation => false,
                };
                if idx > seg_start && is_boundary {
                    segments.push((seg_start, idx));
                    seg_start = idx;
                }
                prev_blank = false;
            }

            idx = if line_end < raw.len() {
                line_end + 1
            } else {
                raw.len()
            };
        }
        if seg_start < raw.len() {
            segments.push((seg_start, raw.len()));
        }

        let last_idx = segments.len().saturating_sub(1);
        let mut items_flushed = list.items_flushed;
        let mut events = Vec::with_capacity(segments.len());

        for (i, (start, end)) in segments.iter().enumerate() {
            let (content, indent) =
                render_list_segment(&raw[*start..*end], list, &mut items_flushed);

            // All segments except the last are complete items (a boundary
            // line followed them); emit as `Block`. The final segment is
            // the partial remainder, emit as `Flush`.
            if i == last_idx {
                events.push(Event::Flush { content, indent });
            } else {
                events.push(Event::Block { content, indent });
            }
        }

        events
    }

    /// Returns the visual indent (in spaces) active for the current state.
    fn current_indent(&self) -> usize {
        match self.state {
            State::InList(list) => list.marker_column,
            State::InFencedCode { indent, .. } => indent,
            _ => self
                .parents
                .iter()
                .rev()
                .find_map(|s| match *s {
                    State::InList(list) => Some(list.marker_column),
                    State::InFencedCode { indent, .. } => Some(indent),
                    _ => None,
                })
                .unwrap_or(0),
        }
    }

    /// Whether the buffered partial first line can only be a paragraph.
    ///
    /// True when the line has fewer than 4 spaces of indent (otherwise it could
    /// be indented code) and its first non-space byte cannot begin any block
    /// starter: header `#`, thematic break / bullet `- * _ +`, fence `` ` `` or
    /// `~`, ordered-list digit, HTML `<`, link reference `[`, or table `|`.
    /// Lets streaming enter `BufferingParagraph` before the first newline so a
    /// single-line paragraph streams instead of stalling until the line ends.
    fn starts_unambiguous_paragraph(&self) -> bool {
        let (indent, content) = get_indent(&self.data);
        if indent >= 4 {
            return false;
        }
        content.bytes().next().is_some_and(|b| {
            !b.is_ascii_digit()
                && !matches!(
                    b,
                    b'#' | b'-' | b'*' | b'_' | b'+' | b'`' | b'~' | b'<' | b'[' | b'|'
                )
        })
    }

    /// Handles the `AtBoundary` state: we are at a block boundary.
    /// We inspect the start of the buffer to decide what block we're in.
    fn handle_at_boundary(&mut self) -> (Option<Event>, State) {
        // Trim leading blank lines, as they are just block separators.
        let trimmed_buffer = self.data.trim_start_matches('\n');

        // If buffer contains only blank lines, wait for more data
        if trimmed_buffer.is_empty() {
            return (None, State::AtBoundary);
        }

        // Trim the blank lines before the content
        let trimmed_len = self.data.len() - trimmed_buffer.len();
        if trimmed_len > 0 {
            self.data.drain(..trimmed_len);
        }

        if self.data.is_empty() {
            return (None, State::AtBoundary);
        }

        // We need at least one full line to decide what block it is.
        let Some(first_line_end) = self.data.find('\n') else {
            // No complete line yet. With streaming on, a partial first line
            // that cannot be the prefix of any block starter is unambiguously
            // a paragraph: enter `BufferingParagraph` now so its prose can
            // stream before the line ends. Otherwise wait for the newline, as
            // a partial prefix can still resolve to a header, fence, list, etc.
            if self.streaming && self.starts_unambiguous_paragraph() {
                return (None, State::BufferingParagraph);
            }
            return (None, State::AtBoundary);
        };

        let first_line = &self.data[..first_line_end];
        let (indent_len, line_content) = get_indent(first_line);

        // Check for "Leaf Blocks" that terminate on one line. Per spec, these
        // blocks may be preceded by up to 3 spaces of indentation

        if indent_len <= 3 && is_atx_header(line_content) {
            let block: String = self.data.drain(..=first_line_end).collect();
            return (Some(Event::block(block)), State::AtBoundary); // Stay at boundary
        }

        if indent_len <= 3 && is_thematic_break(line_content) {
            let block: String = self.data.drain(..=first_line_end).collect();
            return (Some(Event::block(block)), State::AtBoundary); // Stay at boundary
        }

        if indent_len <= 3 && is_link_ref_def(line_content) {
            let block: String = self.data.drain(..=first_line_end).collect();
            return (Some(Event::block(block)), State::AtBoundary); // Stay at boundary
        }

        // Check for "Container Blocks" that change our state. Per spec, these
        // blocks may also be preceded by up to 3 spaces of indentation
        if indent_len <= 3
            && let Some((fence_type, fence_length, language)) = is_fenced_code_start(line_content)
        {
            // Drain the opening line.
            self.data.drain(..=first_line_end);
            return (
                Some(Event::FencedCodeStart {
                    language,
                    fence_type,
                    fence_length,
                    indent: indent_len,
                }),
                State::InFencedCode {
                    fence_type,
                    fence_length,
                    indent: indent_len,
                    depth: 0,
                },
            );
        }

        if indent_len <= 3
            && let Some(rule) = get_html_block_rule(line_content)
        {
            // Change state, return no block.
            return (None, State::InHtmlBlock { block_type: rule });
        }

        // List markers take precedence over indented code: inside a list,
        // 4-space indentation is item content, not a code block. We catch
        // the list at the outer boundary here so the item's first line
        // never gets split off and misclassified later.
        if indent_len <= 3
            && let Some(marker) = parse_list_marker(line_content)
        {
            return (
                None,
                State::InList(ListState {
                    marker_column: indent_len,
                    content_column: indent_len + marker.marker_width,
                    is_ordered: marker.is_ordered,
                    delimiter: marker.delimiter,
                    start_number: marker.number,
                    items_flushed: 0,
                }),
            );
        }

        if indent_len >= 4 && !line_content.is_empty() {
            return (None, State::InIndentedCode);
        }

        // Default Case: Paragraph-like Block. If it's none of the above, it's a
        // paragraph, list, blockquote, etc. Change state, return no block.
        (None, State::BufferingParagraph)
    }

    /// Handles `BufferingParagraph`: we're in a paragraph-like block.
    /// We need to find its terminator.
    fn handle_buffering_paragraph(&mut self) -> (Option<Event>, State) {
        let mut flush_len: Option<usize> = None;
        // For a setext underline terminator, the byte offset just before the
        // underline. A paragraph already streaming splits here (as prose)
        // rather than merging the underline into a heading.
        let mut setext_split: Option<usize> = None;

        // Iterate over all newlines in the buffer to find a terminator.
        for (idx, _) in self.data.match_indices('\n') {
            let line_after_start = idx + 1;

            // Check for blank line (e.g., "...\n\n")
            if self
                .data
                .get(line_after_start..)
                .is_some_and(|s| s.starts_with('\n'))
            {
                flush_len = Some(line_after_start + 1);
                break;
            }

            // Not a blank line. The next line must be complete before it can
            // be classified: a partial prefix can look like a block starter
            // ("#", "<div", "```") while the completed line is not one
            // ("#hello", "<divx>", "```a`b"). Deciding on the prefix would
            // make chunked parsing diverge from whole-document parsing.
            let rest = &self.data[line_after_start..];
            let Some(next_line_end) = rest.find('\n') else {
                continue; // Incomplete next line; wait for more data.
            };

            let (next_line_indent, next_line_content) = get_indent(&rest[..next_line_end]);

            // Check Setext headers first, takes precedence in paragraph
            // context. The underline terminates the paragraph and is included
            // in the flushed block.
            if is_setext_underline(next_line_content) {
                flush_len = Some(line_after_start + next_line_end + 1);
                setext_split = Some(line_after_start);
                break;
            }

            // Interruption by a new block (HTML blocks and < 4 indent code
            // blocks can interrupt)
            if next_line_indent < 4 && is_block_starter(next_line_content) {
                flush_len = Some(line_after_start);
                break;
            }
        }

        if let Some(flush_len) = flush_len {
            return self.flush_paragraph(flush_len, setext_split);
        }

        // No terminator yet. Once the paragraph is too long to be a short
        // setext heading, stream the largest inline-safe prefix of its
        // not-yet-emitted source — unless the block is a GFM table (a pipe-led
        // header), whose column widths make its rendering prefix-unstable.
        if self.streaming && self.data.len() >= SETEXT_STREAM_THRESHOLD && !self.is_table() {
            // Commit only confirmed paragraph lines: never past the last
            // newline, since the in-progress line could still become a block
            // starter. The sole exception is the first line, which is the
            // whole buffer when no newline has arrived yet and which
            // `handle_at_boundary` already classified as a paragraph.
            let block_safe = self.data.rfind('\n').map_or(self.data.len(), |p| p + 1);
            let cut = last_inline_ground_state(&self.data, block_safe);
            if cut > self.para_emitted {
                let content = self.data[self.para_emitted..cut].to_string();
                self.para_emitted = cut;
                return (
                    Some(Event::paragraph_chunk(content, false)),
                    State::BufferingParagraph,
                );
            }
        }

        (None, State::BufferingParagraph)
    }

    /// Whether this block is a GFM table, which must never stream.
    ///
    /// Table rendering is not prefix-stable: a later wide cell re-pads the
    /// header, separator, and earlier rows, so a streamed table would commit
    /// rows that a subsequent row invalidates.
    /// In practice — and in every example in the GFM spec — a table's header
    /// row (the block's first line) begins with a pipe, so a first line whose
    /// first non-space character is `|` is treated as a table.
    /// The check is on the first character, so it never waits for a newline:
    /// ordinary prose with a *mid-line* pipe still streams.
    ///
    /// A pipeless header (`abc | def` with no leading pipe) is permitted by the
    /// spec but absent from its examples and from assistant output; streaming
    /// such a table is a documented limitation.
    fn is_table(&self) -> bool {
        let first_line = self.data.split('\n').next().unwrap_or("");
        get_indent(first_line).1.starts_with('|')
    }

    /// Emit a terminated paragraph.
    ///
    /// When nothing has streamed yet this is a single [`Event::Block`] holding
    /// the paragraph and its terminator (today's behavior).
    /// When the paragraph is mid-stream it is a terminal
    /// [`Event::ParagraphChunk`] carrying the remaining source.
    ///
    /// `flush_len` is the byte count the block path drains.
    /// `setext_split`, when set, is the offset before a setext underline: a
    /// mid-stream paragraph drains only up to there and leaves the underline to
    /// re-parse as a fresh block, so streamed prose is never retroactively
    /// turned into a heading.
    fn flush_paragraph(
        &mut self,
        flush_len: usize,
        setext_split: Option<usize>,
    ) -> (Option<Event>, State) {
        if self.streaming && self.para_emitted > 0 {
            let drain_to = setext_split.unwrap_or(flush_len);
            let content = self.data[self.para_emitted..drain_to].to_string();
            self.data.drain(..drain_to);
            self.para_emitted = 0;
            return (
                Some(Event::paragraph_chunk(content, true)),
                State::AtBoundary,
            );
        }

        let block: String = self.data.drain(..flush_len).collect();
        self.para_emitted = 0;
        (Some(Event::block(block)), State::AtBoundary)
    }

    /// Handles `InList`: we're inside a list, buffering the current item.
    ///
    /// Walks the buffer line by line, looking for a safe flush point.
    /// A flush is safe at:
    ///
    /// - A sibling marker at column == `marker_column` (the current item is
    ///   complete; the new marker starts the next item).
    ///   Stay in this state.
    /// - A line at column ≤ `marker_column` that is not a list marker, when it
    ///   either is a block starter (header, HR, fenced code, HTML block) or
    ///   follows a blank line.
    ///   The list has ended.
    ///   Transition back to `AtBoundary`.
    ///
    /// Blank lines and indented continuations (column \> `marker_column`) are
    /// buffered, not flushed.
    fn handle_in_list(&mut self, list: ListState) -> (Option<Event>, State) {
        let current_state = State::InList(list);

        // Drop a single leading blank line: it belongs to the trailing
        // separator of whatever block was emitted just before us (e.g.
        // a closing fence, or a nested list that terminated on a blank).
        // Multiple blanks are left for the walk's `prev_blank` logic to
        // interpret — two blank lines + less-indented content should
        // still terminate the list.
        //
        // The blank itself carries signal: it means the immediately
        // preceding scope ended on a blank line. Initialise `prev_blank`
        // from that so the walk's terminator check (`prev_blank &&
        // indent < content_column`) fires on the very first line, which
        // matters when a popped child consumed the blank as part of its
        // own flush and left us with a less-indented non-marker at the
        // head of the buffer.
        let leading_blank = leading_blank_line_bytes(&self.data);
        if leading_blank > 0 {
            self.data.drain(..leading_blank);
        }

        // If the buffer starts with a nested marker or a fence at
        // content_column or deeper, we may be re-entering after a flush.
        // Transition to the nested container directly.
        if let Some(transition) = self.maybe_enter_nested_from_list_head(list) {
            self.parents.push(current_state);
            return transition;
        }

        let mut scan = 0_usize;
        // Byte offset just past the last non-blank line we've walked.
        // Used by the `Terminator` branch to flush only the item's
        // actual content and leave trailing blank lines in the buffer,
        // so the popped-to parent state can pick up the same
        // `prev_blank=true` signal that triggered the termination here.
        let mut last_content_end = 0_usize;
        let mut prev_blank = leading_blank > 0;

        while scan < self.data.len() {
            // Compute line shape without holding a borrow on the buffer
            // across the mutable calls below.
            let (line_len, content_is_empty, line_kind) = {
                let slice = &self.data[scan..];
                let Some(newline_pos) = slice.find('\n') else {
                    return (None, current_state);
                };
                let line = &slice[..newline_pos];
                let (indent, content) = get_indent(line);
                let kind = classify_list_line(indent, content, list, prev_blank);
                (newline_pos + 1, content.is_empty(), kind)
            };

            if content_is_empty {
                prev_blank = true;
                scan += line_len;
                continue;
            }

            match line_kind {
                ListLineKind::SiblingMarker => {
                    if scan == 0 {
                        prev_blank = false;
                        scan += line_len;
                        last_content_end = scan;
                        continue;
                    }
                    let (event, new_state) = self.flush_list_segment(scan, scan, list);
                    return (Some(event), new_state);
                }
                ListLineKind::NestedContainer => {
                    if scan > 0 {
                        let (event, new_state) = self.flush_list_segment(scan, scan, list);
                        return (Some(event), new_state);
                    }
                    if let Some(transition) = self.maybe_enter_nested_from_list_head(list) {
                        self.parents.push(current_state);
                        return transition;
                    }
                    // Shouldn't happen given the classifier already
                    // confirmed a nested container is at scan == 0.
                    // Fall through as continuation defensively.
                    prev_blank = false;
                    scan += line_len;
                    last_content_end = scan;
                }
                ListLineKind::Terminator => {
                    let next_state = self.pop_parent_or_boundary();
                    // Flush only this scope's actual content; leave any
                    // trailing blank lines in the buffer so the popped-to
                    // parent state can see them and apply its own
                    // termination check. Without this, a paragraph at
                    // less indent than the parent's `content_column`
                    // would be misclassified as a lazy continuation of
                    // the parent item.
                    if last_content_end == 0 {
                        // Nothing buffered for this list yet (e.g. the
                        // parent's next marker arrived right after we
                        // entered this nested list). Hand control back
                        // to the parent state and let `next()` re-classify
                        // the same line there, rather than emitting an
                        // empty `Block`.
                        return (None, next_state);
                    }
                    // Capture content up to `scan` (including any trailing
                    // blank lines we walked past) so the rendered Block
                    // keeps the visual separator to the next sibling Block;
                    // drain only up to `last_content_end` so those same
                    // blank lines stay in the buffer for the parent state
                    // to see as `prev_blank=true`.
                    let (event, _) = self.flush_list_segment(scan, last_content_end, list);
                    return (Some(event), next_state);
                }
                ListLineKind::Continuation => {
                    prev_blank = false;
                    scan += line_len;
                    last_content_end = scan;
                }
            }
        }

        (None, current_state)
    }

    /// If the buffer starts with a nested list marker or a fence at
    /// `content_column` or deeper, return the transition that enters that
    /// nested container.
    /// The caller is responsible for pushing the current `InList` state onto
    /// `parents` before returning.
    fn maybe_enter_nested_from_list_head(
        &mut self,
        list: ListState,
    ) -> Option<(Option<Event>, State)> {
        let first_line_end = self.data.find('\n')?;
        let first_line = &self.data[..first_line_end];
        let (indent, content) = get_indent(first_line);

        if indent <= list.marker_column || indent < list.content_column {
            return None;
        }

        if let Some(marker) = parse_list_marker(content) {
            let nested = State::InList(ListState {
                marker_column: indent,
                content_column: indent + marker.marker_width,
                is_ordered: marker.is_ordered,
                delimiter: marker.delimiter,
                start_number: marker.number,
                items_flushed: 0,
            });
            return Some((None, nested));
        }

        if let Some((fence_type, fence_length, language)) = is_fenced_code_start(content) {
            let _drained = self.data.drain(..=first_line_end);
            return Some((
                Some(Event::FencedCodeStart {
                    language,
                    fence_type,
                    fence_length,
                    indent,
                }),
                State::InFencedCode {
                    fence_type,
                    fence_length,
                    indent,
                    depth: 0,
                },
            ));
        }

        None
    }

    /// Capture `content_end` bytes as the Block content and drain `drain_end`
    /// bytes from the buffer.
    ///
    /// In the common case (`SiblingMarker`, `NestedContainer`), the two are
    /// equal: drain the same bytes that go into the Block.
    /// The `Terminator` branch passes `content_end > drain_end` to keep
    /// trailing blank lines *both* in the emitted Block (so the renderer
    /// preserves the visual separation between this item and whatever follows)
    /// *and* in the buffer (so the popped-to parent state can pick up
    /// `prev_blank=true`).
    ///
    /// Stripping and renumbering are delegated to [`render_list_segment`]; the
    /// returned state carries the updated `items_flushed`.
    fn flush_list_segment(
        &mut self,
        content_end: usize,
        drain_end: usize,
        list: ListState,
    ) -> (Event, State) {
        debug_assert!(
            drain_end <= content_end,
            "drain_end ({drain_end}) must not exceed content_end ({content_end})"
        );
        let raw: String = self.data[..content_end].to_string();
        self.data.drain(..drain_end);

        let mut items_flushed = list.items_flushed;
        let (content, indent) = render_list_segment(&raw, list, &mut items_flushed);

        let new_state = State::InList(ListState {
            items_flushed,
            ..list
        });
        (Event::Block { content, indent }, new_state)
    }

    /// Handles `InIndentedCode`: we're looking for the end of an indented code
    /// block.
    fn handle_in_indented_code(&mut self) -> (Option<Event>, State) {
        let mut scan_pos = 0;
        let mut block_len = 0;
        let mut terminated = false;

        while scan_pos < self.data.len() {
            let slice = &self.data[scan_pos..];
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
            let block: String = self.data.drain(..block_len).collect();
            return (Some(Event::block(block)), State::AtBoundary);
        }

        (None, State::InIndentedCode)
    }

    /// Returns `Some(true)` if the blank line at `start_idx` is followed by an
    /// indented code line, `Some(false)` if it is followed by less-indented
    /// content, and `None` if more data is needed.
    fn indented_code_blank_followed_by_indented(&self, start_idx: usize) -> Option<bool> {
        let mut idx = start_idx;

        while idx < self.data.len() {
            let slice = &self.data[idx..];
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
    ///
    /// Tracks nesting depth so that inner fenced code blocks (which LLMs
    /// frequently produce inside markdown code blocks) don't prematurely close
    /// the outer block.
    fn handle_in_fenced_code(
        &mut self,
        fence_type: FenceType,
        fence_length: usize,
        indent: usize,
        depth: usize,
    ) -> (Option<Event>, State) {
        let current_state = State::InFencedCode {
            fence_type,
            fence_length,
            indent,
            depth,
        };

        // We need at least one newline to have a full line
        let Some(line_end) = self.data.find('\n') else {
            return (None, current_state);
        };

        let line_content_slice = &self.data[..line_end];
        let (indent_len, content) = get_indent(line_content_slice);

        let expected_char = fence_type.as_char();

        // Check if this line looks like a fence (opening or closing).
        //
        // Per CommonMark §4.5, a fence may be indented up to 3 spaces
        // *relative to its container*. For document-level fences the
        // container is column 0 and the stored `indent` is 0, so this is
        // equivalent to the old `indent_len < 4` rule. For fences nested
        // inside list items, `indent` is the opening fence's visual
        // column (e.g. 4 for an item with marker `10. `), so the close
        // is allowed in `[indent, indent + 3]` — otherwise the closing
        // fence sits at exactly `indent` and would be misclassified as a
        // code line, leaving the buffer stuck in `InFencedCode` forever.
        let fence_on_line =
            if indent_len.saturating_sub(indent) < 4 && content.starts_with(expected_char) {
                let run = content.chars().take_while(|&c| c == expected_char).count();
                if run >= fence_length {
                    let after = &content[run..];
                    Some((run, after.trim()))
                } else {
                    None
                }
            } else {
                None
            };

        if let Some((_run, after)) = fence_on_line {
            if after.is_empty() {
                // Bare closing fence.
                if depth == 0 {
                    // This closes the outer block. If we were inside a
                    // list item, pop back to the parent; otherwise go
                    // back to `AtBoundary`.
                    self.data.drain(..=line_end);
                    let fence = expected_char.to_string().repeat(fence_length);
                    let next_state = self.pop_parent_or_boundary();
                    return (Some(Event::FencedCodeEnd { fence, indent }), next_state);
                }

                // Closes an inner block — decrement depth, emit as code.
                let mut raw_line = self.data.drain(..=line_end).collect::<String>();
                strip_indent(&mut raw_line, indent);
                let next = State::InFencedCode {
                    fence_type,
                    fence_length,
                    indent,
                    depth: depth - 1,
                };
                return (
                    Some(Event::FencedCodeLine {
                        content: raw_line,
                        indent,
                    }),
                    next,
                );
            }

            // Has content after the backticks — looks like a nested opening.
            let mut raw_line = self.data.drain(..=line_end).collect::<String>();
            strip_indent(&mut raw_line, indent);
            let next = State::InFencedCode {
                fence_type,
                fence_length,
                indent,
                depth: depth + 1,
            };
            return (
                Some(Event::FencedCodeLine {
                    content: raw_line,
                    indent,
                }),
                next,
            );
        }

        // Regular code line.
        let mut raw_line = self.data.drain(..=line_end).collect::<String>();
        strip_indent(&mut raw_line, indent);

        (
            Some(Event::FencedCodeLine {
                content: raw_line,
                indent,
            }),
            current_state,
        )
    }

    /// Handles `InHtmlBlock`: look for termination based on its rule.
    fn handle_in_html_block(&mut self, block_type: HtmlBlockRule) -> (Option<Event>, State) {
        let current_state = State::InHtmlBlock { block_type };
        let terminator = match block_type.terminator() {
            // The block runs through the end of the blank line.
            HtmlTerminator::BlankLine => self.data.find("\n\n").map(|pos| pos + "\n\n".len()),
            // The block runs through the end of the line containing the tag.
            HtmlTerminator::Tag(tag) => self.data.find(tag).and_then(|tag_pos| {
                self.data[tag_pos..]
                    .find('\n')
                    .map(|line_end| tag_pos + line_end + 1)
            }),
        };

        terminator
            .map(|pos| self.data.drain(..pos).collect())
            .map_or((None, current_state), |block: String| {
                (Some(Event::block(block)), State::AtBoundary)
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
                State::InList(list) => self.handle_in_list(list),
                State::InFencedCode {
                    fence_type,
                    fence_length,
                    indent,
                    depth,
                } => self.handle_in_fenced_code(fence_type, fence_length, indent, depth),
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

/// Check if content (after indent stripping) starts with a list marker.
///
/// Matches unordered (` -  `, ` *  `, ` +  `) and ordered (` 1.  `, ` 2)  `)
/// markers.
fn is_list_marker(content: &str) -> bool {
    parse_list_marker(content).is_some()
}

/// A parsed list marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ListMarker {
    /// Visual width of the marker including the trailing space, e.g. 2 for `-`,
    /// 3 for ` 1.  `, 4 for ` 10.  `.
    marker_width: usize,
    /// Whether the marker is ordered (digits + delimiter).
    is_ordered: bool,
    /// The delimiter byte: `.` or `)` for ordered, `-`/`*`/`+` for bullet.
    delimiter: u8,
    /// For ordered markers, the number value.
    /// `0` for bullet markers.
    number: u32,
}

/// Parse a list marker at the start of `content`, returning its shape if
/// present.
/// `content` should already have leading whitespace stripped.
fn parse_list_marker(content: &str) -> Option<ListMarker> {
    let bytes = content.as_bytes();

    // Bullet markers: `-`, `*`, `+` followed by a space.
    if let [c @ (b'-' | b'*' | b'+'), b' ', ..] = bytes {
        return Some(ListMarker {
            marker_width: 2,
            is_ordered: false,
            delimiter: *c,
            number: 0,
        });
    }

    // Ordered markers: one or more digits, then `.` or `)`, then a space.
    let digit_count = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
    if digit_count == 0 || digit_count >= bytes.len() {
        return None;
    }
    let delim = bytes[digit_count];
    if (delim != b'.' && delim != b')') || bytes.get(digit_count + 1) != Some(&b' ') {
        return None;
    }

    let number = content.get(..digit_count).and_then(|s| s.parse().ok())?;
    Some(ListMarker {
        marker_width: digit_count + 2,
        is_ordered: true,
        delimiter: delim,
        number,
    })
}

/// Count the number of leading bytes in `s` that form *a single* blank line
/// (only spaces and tabs, terminated by `\n`).
/// Returns `0` if the content doesn't begin with a blank line.
///
/// Used at the start of `handle_in_list` to consume the trailing separator left
/// behind by a just-closed inner block (e.g. a fenced code block).
/// Stops after one line so two-blank-lines-end-of-list semantics still
/// propagate to the walk.
fn leading_blank_line_bytes(s: &str) -> usize {
    let bytes = s.as_bytes();
    let mut idx = 0;
    while idx < bytes.len() && (bytes[idx] == b' ' || bytes[idx] == b'\t') {
        idx += 1;
    }
    if idx < bytes.len() && bytes[idx] == b'\n' {
        idx + 1
    } else {
        0
    }
}

/// Classification of a line encountered while in `InList`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListLineKind {
    /// A sibling marker at this list's `marker_column`.
    SiblingMarker,
    /// A list marker or fenced code start at `content_column` or deeper — a
    /// nested container inside the current item.
    NestedContainer,
    /// A line that terminates the list: less-indented after a blank, or a block
    /// interrupter at \<= 3 spaces.
    Terminator,
    /// Any other non-blank line: continuation of the current item.
    Continuation,
}

/// Classify a non-blank line for `handle_in_list` and `flush_list_events`.
///
/// `list` describes the active list's marker shape, used to distinguish sibling
/// markers from markers that start a *new* list at the same column (per
/// CommonMark §5.2: two markers are the same kind only if they share
/// `is_ordered` and their delimiter character).
fn classify_list_line(
    indent: usize,
    content: &str,
    list: ListState,
    prev_blank: bool,
) -> ListLineKind {
    if content.is_empty() {
        return ListLineKind::Continuation;
    }

    let marker = parse_list_marker(content);
    let is_marker = marker.is_some();
    let is_fence = is_fenced_code_start(content).is_some();

    // A marker is only a sibling of the current list if it matches the
    // current list's kind (ordered vs bullet) and uses the same delimiter
    // character. A mismatched marker at the same column starts a new list.
    let is_sibling =
        marker.is_some_and(|m| m.is_ordered == list.is_ordered && m.delimiter == list.delimiter);

    if indent == list.marker_column && is_sibling {
        return ListLineKind::SiblingMarker;
    }

    if indent >= list.content_column && (is_marker || is_fence) {
        return ListLineKind::NestedContainer;
    }

    let is_block_interrupter = indent <= 3 && is_block_starter(content);
    // A list marker at less indent than this list's content column always
    // terminates the current list — either it belongs to an enclosing list
    // / document level (indent below `marker_column`), or it sits at the
    // current `marker_column` with a different type or delimiter, starting
    // a new list per CommonMark §5.2 (since `marker_column < content_column`
    // is always true, this case also has `indent < content_column`). The
    // sibling-shape check above only catches *matching* markers at
    // `marker_column`; mismatched ones fall through to here. Non-marker
    // lines only terminate after a blank line, otherwise they're lazy
    // continuation of the current paragraph.
    let is_outer_marker = is_marker && indent < list.content_column;
    if (indent < list.content_column && prev_blank) || is_outer_marker || is_block_interrupter {
        return ListLineKind::Terminator;
    }

    ListLineKind::Continuation
}

/// Strip and (when applicable) renumber a single list segment for emission.
///
/// The segment's first line decides the treatment:
///
/// - A *sibling* marker of `list` at the marker column: strip the marker
///   column's indent, renumber ordered markers against `start_number +
///   items_flushed`, and increment `items_flushed`.
/// - A *foreign* marker at the marker column (different kind or delimiter, i.e.
///   the start of a new list): strip the marker column's indent, leaving the
///   marker untouched.
/// - Anything else is continuation content inside the current item: strip the
///   content column's indent.
///
/// Returns the rendered content and the visual indent the consumer should
/// apply.
/// Shared by the streaming path (`flush_list_segment`) and the end-of-region
/// path (`flush_list_events`) so both agree on stripping and renumbering.
fn render_list_segment(segment: &str, list: ListState, items_flushed: &mut u32) -> (String, usize) {
    let first_line = segment.lines().next().unwrap_or("");
    let (first_indent, first_content) = get_indent(first_line);
    let marker = (first_indent == list.marker_column)
        .then(|| parse_list_marker(first_content))
        .flatten();

    let Some(marker) = marker else {
        return (
            strip_lines_indent(segment, list.content_column),
            list.content_column,
        );
    };

    let stripped = strip_lines_indent(segment, list.marker_column);
    let is_sibling = marker.is_ordered == list.is_ordered && marker.delimiter == list.delimiter;
    if !is_sibling {
        return (stripped, list.marker_column);
    }

    let content = if list.is_ordered {
        renumber_first_marker(stripped, list.start_number + *items_flushed, list.delimiter)
    } else {
        stripped
    };
    *items_flushed += 1;
    (content, list.marker_column)
}

/// Strip up to `max_strip` leading spaces from `line`.
fn strip_indent(line: &mut String, max_strip: usize) {
    let leading = line.chars().take_while(|&c| c == ' ').count();
    let strip = leading.min(max_strip);
    if strip > 0 {
        line.drain(..strip);
    }
}

/// Strip up to `max_strip` leading spaces from each line of `raw`.
fn strip_lines_indent(raw: &str, max_strip: usize) -> String {
    if max_strip == 0 {
        return raw.to_string();
    }
    let mut out = String::with_capacity(raw.len());
    for line in raw.split_inclusive('\n') {
        // Skip up to max_strip leading spaces.
        let leading = line
            .chars()
            .take_while(|&c| c == ' ')
            .count()
            .min(max_strip);
        out.push_str(&line[leading..]);
    }
    out
}

/// Rewrite the leading ordered-list marker number in `content` to `new`.
///
/// `delimiter` is the marker's delimiter byte (`.` or `)`); used to confirm the
/// leading marker shape before rewriting.
/// If the content does not start with a matching marker, it is returned
/// unchanged.
fn renumber_first_marker(content: String, new: u32, delimiter: u8) -> String {
    let bytes = content.as_bytes();
    let digit_count = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
    if digit_count == 0 || digit_count >= bytes.len() || bytes[digit_count] != delimiter {
        return content;
    }
    let new_str = new.to_string();
    if new_str.as_bytes() == &bytes[..digit_count] {
        return content;
    }
    let mut out = String::with_capacity(content.len() + new_str.len());
    out.push_str(&new_str);
    out.push_str(&content[digit_count..]);
    out
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

/// Largest byte offset `<= limit` at which `s[..offset]` is at inline ground
/// state: every inline construct opened inside it is also closed inside it.
///
/// Paragraph streaming uses this to avoid committing a chunk that ends inside
/// an open inline construct, whose later close could reflow earlier (already
/// emitted) output.
/// The scan is deliberately one-sided — it pushes on every *potential* opener
/// and pops only on a confident match — so it may over-hold (return a smaller
/// offset, costing a little latency) but never under-holds.
/// The enabled extensions are tracked: `` ` `` (code), `*` `_` `~` `^`
/// (emphasis / strikethrough / sub- and superscript), `[` `]` (links / images,
/// with `](`, `][` and end-of-buffer lookahead), and `<` (autolink / raw HTML).
fn last_inline_ground_state(s: &str, limit: usize) -> usize {
    let bytes = s.as_bytes();
    let limit = limit.min(bytes.len());

    // Open emphasis / bracket / angle markers. Ground state requires this empty
    // and no open code span.
    let mut stack: Vec<u8> = Vec::new();
    // Backtick run length of the open code span, if any.
    let mut code_span: Option<usize> = None;
    let mut last_ground = 0;
    let mut i = 0;

    while i < limit {
        let b = bytes[i];

        // Inside a code span nothing else applies; only a backtick run of the
        // same length closes it.
        if let Some(open_len) = code_span {
            if b == b'`' {
                let run = ascii_run(bytes, i, b'`', limit);
                if run == open_len {
                    code_span = None;
                }
                i += run;
            } else {
                i += char_width(s, i);
            }
            if code_span.is_none() && stack.is_empty() {
                last_ground = i;
            }
            continue;
        }

        match b {
            b'`' => {
                let run = ascii_run(bytes, i, b'`', limit);
                code_span = Some(run);
                i += run;
            }
            // Hold a `<` only when it could begin an autolink or HTML tag; a
            // literal `<` (e.g. "a < b") is left as content.
            b'<' => {
                if bytes
                    .get(i + 1)
                    .is_some_and(|&n| n.is_ascii_alphabetic() || matches!(n, b'/' | b'!' | b'?'))
                {
                    stack.push(b'<');
                }
                i += 1;
            }
            b'>' => {
                if stack.last() == Some(&b'<') {
                    stack.pop();
                }
                i += 1;
            }
            b'[' => {
                stack.push(b'[');
                i += 1;
            }
            b']' => {
                if stack.last() == Some(&b'[') {
                    match bytes.get(i + 1) {
                        // `](` opens a link destination: swap the bracket for a
                        // paren that closes on `)`.
                        Some(b'(') => {
                            stack.pop();
                            stack.push(b'(');
                        }
                        // `][` (a reference) or `]` at the buffer edge stay
                        // ambiguous: keep holding until the next byte resolves
                        // them.
                        Some(b'[') | None => {}
                        // Plain `]` closes the bracket.
                        _ => {
                            stack.pop();
                        }
                    }
                }
                i += 1;
            }
            b')' => {
                if stack.last() == Some(&b'(') {
                    stack.pop();
                }
                i += 1;
            }
            b'*' | b'_' | b'~' | b'^' => {
                let run = ascii_run(bytes, i, b, limit);
                let prev = i.checked_sub(1).map(|p| bytes[p]);
                let next = bytes.get(i + run).copied();
                // `_` cannot open or close intra-word, so "foo_bar" is literal;
                // the others may.
                let open_ok = b != b'_' || prev.is_none_or(|p| !p.is_ascii_alphanumeric());
                let close_ok = b != b'_' || next.is_none_or(|n| !n.is_ascii_alphanumeric());
                let can_open = open_ok && next.is_none_or(|n| !n.is_ascii_whitespace());
                let can_close = close_ok && prev.is_some_and(|p| !p.is_ascii_whitespace());
                if can_close && stack.last() == Some(&b) {
                    stack.pop();
                } else if can_open {
                    stack.push(b);
                }
                i += run;
            }
            _ => i += char_width(s, i),
        }

        if code_span.is_none() && stack.is_empty() {
            last_ground = i;
        }
    }

    last_ground
}

/// Count consecutive `ch` bytes in `bytes[start..limit]`.
fn ascii_run(bytes: &[u8], start: usize, ch: u8, limit: usize) -> usize {
    let mut n = 0;
    while start + n < limit && bytes[start + n] == ch {
        n += 1;
    }
    n
}

/// Byte width of the UTF-8 character beginning at `s[i]` (at least 1).
fn char_width(s: &str, i: usize) -> usize {
    s[i..].chars().next().map_or(1, char::len_utf8)
}

#[cfg(test)]
#[path = "buffer_tests.rs"]
mod tests;
