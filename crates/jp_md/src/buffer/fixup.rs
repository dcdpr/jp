//! Post-processing fixups for buffer events.
//!
//! Fixups are stateful transformers that sit between the [`Buffer`] iterator
//! and the consumer.
//! They handle LLM-specific quirks that don't belong in the core markdown
//! parsing logic.
//!
//! # Streaming constraint
//!
//! A fixup must not retroactively rewrite a block's already-emitted bytes based
//! on later content.
//! With paragraph streaming on, a top-level paragraph's prose is emitted as
//! [`Event::ParagraphChunk`]s and rendered before the paragraph ends, so
//! anything a fixup would change after the fact has already been printed.
//! The current fixups satisfy this because they only rewrite *following*
//! events: [`OrphanedFenceFixup`] derives its embedded-fence flag from the
//! accumulated paragraph and acts on the *next* bare fence,
//! [`FenceEscalationFixup`] touches only fenced-code events, and
//! [`SplitCodeSpanFixup`] rewrites only the leading prefix of the paragraph
//! *after* a dangling opener, before any of it has rendered.
//! A future fixup that repairs paragraph prose from whole-paragraph context
//! must run in the buffer before streaming, or opt the affected paragraph out
//! of streaming.
//!
//! [`Buffer`]: super::Buffer
//! [`Event::ParagraphChunk`]: super::Event::ParagraphChunk

use super::Event;

/// A stateful event transformer.
///
/// Each fixup inspects events as they pass through and can rewrite, suppress,
/// or pass them unchanged.
/// Fixups may hold state across events (e.g. remembering properties of the
/// previous block).
pub trait EventFixup {
    /// Process a single event.
    /// Returns `None` to suppress the event, or `Some(event)` (possibly
    /// modified) to pass it through.
    fn process(&mut self, event: Event) -> Option<Event>;
}

/// An ordered set of [`EventFixup`]s applied to each event.
#[derive(Default)]
pub struct Fixups {
    /// Fixups applied in order to each event.
    fixups: Vec<Box<dyn EventFixup>>,
}

impl Fixups {
    /// Create a set from the given fixups.
    /// Fixups are applied in the order given.
    #[must_use]
    pub fn new(fixups: Vec<Box<dyn EventFixup>>) -> Self {
        Self { fixups }
    }

    /// The fixup set applied to LLM output: orphaned-fence correction, fence
    /// escalation, and split code-span repair.
    #[must_use]
    pub fn llm_quirks() -> Self {
        Self::new(vec![
            Box::new(OrphanedFenceFixup::new()),
            Box::new(FenceEscalationFixup),
            Box::new(SplitCodeSpanFixup::new()),
        ])
    }

    /// Run an event through all fixups in order.
    /// Returns `None` if any fixup suppressed the event.
    pub fn apply(&mut self, event: Event) -> Option<Event> {
        self.fixups
            .iter_mut()
            .try_fold(event, |event, fixup| fixup.process(event))
    }
}

/// Check if a block contains a fence pattern embedded mid-line (not at the
/// start).
/// This indicates the LLM started a code block at the end of a paragraph line,
/// and a subsequent bare fence is likely the orphaned close.
fn has_embedded_fence(block: &str) -> bool {
    for line in block.lines() {
        let trimmed = line.trim_start();
        // Skip lines that start with a fence char (those are proper fences).
        if trimmed.starts_with('`') || trimmed.starts_with('~') {
            continue;
        }
        // Look for 3+ consecutive backticks or tildes after other content.
        if trimmed.contains("```") || trimmed.contains("~~~") {
            return true;
        }
    }
    false
}

/// Fixes orphaned closing fences from mid-line code fence patterns.
///
/// When an LLM produces backticks mid-line (e.g. `text:```lang`), the bare
/// closing fence on the next line gets misinterpreted as a new code block
/// opening.
/// This fixup detects when a `Block` contains such an embedded fence pattern
/// and converts the following bare `FencedCodeStart` (no language tag) into a
/// `Block` instead.
pub struct OrphanedFenceFixup {
    /// Whether the previous block had an embedded fence pattern.
    prev_had_embedded_fence: bool,
    /// When true, we're inside a fake code block from an orphaned fence.
    /// All `FencedCodeLine` events become `Block` events, and `FencedCodeEnd`
    /// is suppressed.
    suppressing: bool,
    /// Source of the streamed paragraph in flight.
    ///
    /// The embedded-fence check is per *line*, but the inline scanner commits
    /// the prose before an embedded fence in an earlier chunk and holds the
    /// fence run into a later one, so a per-chunk check would see the fence at
    /// a chunk's start and mistake it for a proper (line-leading) fence.
    /// The flag is therefore computed over the whole accumulated paragraph at
    /// the terminal chunk.
    paragraph_buf: String,
    /// Whether a streamed paragraph is mid-flight.
    in_paragraph: bool,
}

impl Default for OrphanedFenceFixup {
    fn default() -> Self {
        Self::new()
    }
}

impl OrphanedFenceFixup {
    /// Create a new fixup.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            prev_had_embedded_fence: false,
            suppressing: false,
            paragraph_buf: String::new(),
            in_paragraph: false,
        }
    }
}

impl EventFixup for OrphanedFenceFixup {
    fn process(&mut self, event: Event) -> Option<Event> {
        // While suppressing a fake code block, convert lines to blocks
        // and swallow the closing fence.
        if self.suppressing {
            return match event {
                Event::FencedCodeLine { content, indent } => Some(Event::Block { content, indent }),
                Event::FencedCodeEnd { .. } => {
                    self.suppressing = false;
                    None
                }
                other => Some(other),
            };
        }

        match &event {
            Event::Block { content, .. } => {
                self.prev_had_embedded_fence = has_embedded_fence(content);
                Some(event)
            }
            // A streamed paragraph stands in for a `Block`: accumulate its
            // source and compute the embedded-fence flag over the whole
            // paragraph at the terminal chunk, so the flag is ready for the
            // block that follows. A per-chunk check would miss a fence the
            // scanner pushed to a chunk's start (the prose before it committed
            // earlier), seeing it as a line-leading fence.
            Event::ParagraphChunk { content, last, .. } => {
                if !self.in_paragraph {
                    self.paragraph_buf.clear();
                    self.prev_had_embedded_fence = false;
                    self.in_paragraph = true;
                }
                self.paragraph_buf.push_str(content);
                if *last {
                    self.prev_had_embedded_fence = has_embedded_fence(&self.paragraph_buf);
                    self.paragraph_buf.clear();
                    self.in_paragraph = false;
                }
                Some(event)
            }
            Event::FencedCodeStart {
                language, indent, ..
            } if self.prev_had_embedded_fence && language.is_empty() => {
                self.prev_had_embedded_fence = false;
                self.suppressing = true;
                // Convert the fence itself to a block.
                Some(Event::Block {
                    content: format!("{event}\n"),
                    indent: *indent,
                })
            }
            _ => {
                self.prev_had_embedded_fence = false;
                Some(event)
            }
        }
    }
}

/// Escalates fence lengths so rendered output safely contains inner fences.
///
/// Rewrites `FencedCodeStart` and `FencedCodeEnd` events to use at least 5
/// backticks/tildes, so 3-backtick inner fences render as literal content in
/// the output.
pub struct FenceEscalationFixup;

impl EventFixup for FenceEscalationFixup {
    fn process(&mut self, event: Event) -> Option<Event> {
        match event {
            Event::FencedCodeStart {
                language,
                fence_type,
                fence_length,
                indent,
            } => Some(Event::FencedCodeStart {
                language,
                fence_type,
                fence_length: fence_length.max(5),
                indent,
            }),
            Event::FencedCodeEnd { fence, indent } => {
                let ch = fence.chars().next().unwrap_or('`');
                let len = fence.len().max(5);
                Some(Event::FencedCodeEnd {
                    fence: ch.to_string().repeat(len),
                    indent,
                })
            }
            other => Some(other),
        }
    }
}

/// Maximum bytes of whitespace-free prefix searched for an orphaned closer.
///
/// An injected paragraph break splits a single token, so the closing run must
/// appear within the first "word" of the following paragraph.
/// The cap bounds the search for pathological single-word paragraphs; widen it
/// if a real-world split ever exceeds it.
const MAX_CLOSER_PREFIX: usize = 64;

/// Repairs inline code spans split by a paragraph break injected mid-span.
///
/// LLM streams occasionally emit a spurious blank line inside an inline code
/// span (`` `_rfd-next- `` + blank line + `` number` ``).
/// CommonMark ends the paragraph at the blank line, so the opener renders as a
/// harmless literal backtick, but the orphaned closer at the start of the next
/// paragraph shifts every backtick pairing after it off by one, styling prose
/// as code and code as prose.
///
/// The fixup tracks whether a paragraph ends inside an open code span
/// (run-length aware, mirroring comrak's pairing rules).
/// When the immediately following paragraph presents a backtick run of the same
/// length within its leading whitespace-free prefix, that run is taken as the
/// orphaned closer and backslash-escaped: it renders as a literal backtick and
/// restores the pairing of every span after it.
/// The dangling opener itself needs no repair, because an unpaired opener
/// already renders literally.
///
/// Misfire guards, in rough order of selectivity:
///
/// - Only the immediately following paragraph-like event ([`Event::Block`],
///   [`Event::Flush`], or the first [`Event::ParagraphChunk`]s of a paragraph)
///   participates; fenced-code events disarm the state.
/// - The closer run must match the opener's run length exactly; shorter or
///   longer runs are literal content of the split span and are skipped.
/// - It must appear before any whitespace, within [`MAX_CLOSER_PREFIX`] bytes
///   of the paragraph start.
/// - A run at offset zero is only treated as a closer when followed by
///   whitespace, so a legitimate `` `foo` `` span at paragraph start is left
///   alone.
///
/// The rewrite happens in the paragraph's leading chunk before it is rendered,
/// so the streaming constraint above holds.
pub struct SplitCodeSpanFixup {
    /// Run length of the unclosed opener that ended the previous paragraph.
    armed: Option<usize>,
    /// Whether a streamed paragraph is mid-flight.
    in_paragraph: bool,
    /// Open code span run length carried across the current paragraph's chunks
    /// (computed over the *repaired* content).
    open: Option<usize>,
    /// Search for the orphaned closer in the current paragraph's prefix.
    search: Search,
}

/// Closer-search state for the paragraph following a dangling opener.
enum Search {
    /// Not searching.
    Off,
    /// Scanning the leading whitespace-free prefix for a `run`-length backtick
    /// run; `seen` counts prefix bytes consumed by earlier chunks.
    Active {
        /// The dangling opener's backtick run length.
        run: usize,
        /// Prefix bytes consumed by earlier chunks of this paragraph.
        seen: usize,
    },
}

impl Default for SplitCodeSpanFixup {
    fn default() -> Self {
        Self::new()
    }
}

impl SplitCodeSpanFixup {
    /// Create a new fixup.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            armed: None,
            in_paragraph: false,
            open: None,
            search: Search::Off,
        }
    }
}

impl EventFixup for SplitCodeSpanFixup {
    fn process(&mut self, event: Event) -> Option<Event> {
        match event {
            Event::ParagraphChunk {
                mut content,
                indent,
                last,
            } => {
                if !self.in_paragraph {
                    self.in_paragraph = true;
                    self.open = None;
                    self.search = self
                        .armed
                        .take()
                        .map_or(Search::Off, |run| Search::Active { run, seen: 0 });
                }
                if let Search::Active { run, mut seen } = self.search {
                    self.search = if repair_orphaned_closer(&mut content, run, &mut seen) {
                        Search::Off
                    } else {
                        Search::Active { run, seen }
                    };
                }
                self.open = scan_code_spans(&content, self.open);
                if last {
                    self.armed = self.open.take();
                    self.in_paragraph = false;
                    self.search = Search::Off;
                }
                Some(Event::ParagraphChunk {
                    content,
                    indent,
                    last,
                })
            }
            Event::Block {
                mut content,
                indent,
            } => {
                // A `Block` stands in for a whole paragraph; clear any stray
                // streamed-paragraph state.
                self.in_paragraph = false;
                self.open = None;
                self.search = Search::Off;
                if let Some(run) = self.armed.take() {
                    let mut seen = 0;
                    let _ = repair_orphaned_closer(&mut content, run, &mut seen);
                }
                self.armed = scan_code_spans(&content, None);
                Some(Event::Block { content, indent })
            }
            Event::Flush {
                mut content,
                indent,
            } => {
                // A short trailing paragraph that never began streaming
                // reaches the consumer as a `Flush` (`flush_events` only
                // emits a terminal chunk for a paragraph that already
                // streamed), so the orphaned closer is repaired here too.
                // A flush is a region boundary, though: nothing after it
                // continues this content, so it never arms the fixup.
                self.in_paragraph = false;
                self.open = None;
                self.search = Search::Off;
                if let Some(run) = self.armed.take() {
                    let mut seen = 0;
                    let _ = repair_orphaned_closer(&mut content, run, &mut seen);
                }
                Some(Event::Flush { content, indent })
            }
            // Any other event breaks the paragraph continuation.
            other => {
                self.armed = None;
                self.in_paragraph = false;
                self.open = None;
                self.search = Search::Off;
                Some(other)
            }
        }
    }
}

/// Search `content` (a leading portion of a paragraph) for the orphaned closer
/// of a dangling `run`-length opener, escaping it in place when found.
///
/// Returns `true` when the search resolved — a *matching* backtick run,
/// whitespace, or the prefix cap was reached — whether or not a repair was
/// made.
/// A mismatched run is literal content of the split span (a `run`-length span
/// closes only on an exact-length run), so the search skips it and continues.
/// Returns `false` when `content` ended while still inside the whitespace-free
/// prefix, in which case the search continues in the next chunk with `seen`
/// advanced.
fn repair_orphaned_closer(content: &mut String, run: usize, seen: &mut usize) -> bool {
    let mut i = 0;
    while i < content.len() {
        if *seen + i >= MAX_CLOSER_PREFIX {
            return true;
        }
        let b = content.as_bytes()[i];
        if b == b'`' {
            let len = backtick_run(content.as_bytes(), i);
            if len == run {
                // A run at offset zero is ambiguous with a legitimate span
                // opener; only a following whitespace marks it as a closer.
                // Either way a matching run resolves the search: scanning
                // past a declined match could escape the true closer of a
                // legitimate leading span.
                let followed_by_ws = content[i + len..]
                    .chars()
                    .next()
                    .is_none_or(char::is_whitespace);
                if *seen + i > 0 || followed_by_ws {
                    content.replace_range(i..i + len, &"\\`".repeat(len));
                }
                return true;
            }
            i += len;
            continue;
        }
        if b == b'\\' {
            // A backslash-escaped character is ordinary prefix content.
            i += 1;
            if let Some(c) = content[i..].chars().next() {
                i += c.len_utf8();
            }
            continue;
        }
        let c = content[i..].chars().next().unwrap_or('\0');
        if c.is_whitespace() {
            return true;
        }
        i += c.len_utf8();
    }
    *seen += content.len();
    false
}

/// Scan `content` for inline code spans, starting from an already-`open` span's
/// run length (or `None` at paragraph start), and return the open span's run
/// length at the end.
///
/// Mirrors comrak's pairing: a backtick run opens a span, and only a run of the
/// exact same length closes it; a backslash-escaped backtick outside a span is
/// literal.
fn scan_code_spans(content: &str, mut open: Option<usize>) -> Option<usize> {
    let bytes = content.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match (open, bytes[i]) {
            (Some(len), b'`') => {
                let run = backtick_run(bytes, i);
                if run == len {
                    open = None;
                }
                i += run;
            }
            (None, b'`') => {
                open = Some(backtick_run(bytes, i));
                i += open.unwrap_or(1);
            }
            // Skip the backslash and the escaped byte. Landing inside a
            // multi-byte character is harmless: continuation bytes never
            // match an ASCII backtick or backslash.
            (None, b'\\') => i += 2,
            _ => i += 1,
        }
    }
    open
}

/// Count consecutive backticks in `bytes` starting at `start`.
fn backtick_run(bytes: &[u8], start: usize) -> usize {
    bytes[start..].iter().take_while(|&&b| b == b'`').count()
}

#[cfg(test)]
#[path = "fixup_tests.rs"]
mod tests;
