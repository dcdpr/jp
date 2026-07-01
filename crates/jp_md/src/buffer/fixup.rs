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
//! The current fixups satisfy this because they only rewrite *following* fence
//! events: [`OrphanedFenceFixup`] derives its embedded-fence flag from the
//! accumulated paragraph and acts on the *next* bare fence, and
//! [`FenceEscalationFixup`] touches only fenced-code events.
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

    /// The fixup set applied to LLM output: orphaned-fence correction and fence
    /// escalation.
    #[must_use]
    pub fn llm_quirks() -> Self {
        Self::new(vec![
            Box::new(OrphanedFenceFixup::new()),
            Box::new(FenceEscalationFixup),
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

#[cfg(test)]
#[path = "fixup_tests.rs"]
mod tests;
