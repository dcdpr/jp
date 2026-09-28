//! Display sanitization of untrusted content bound for the terminal.
//!
//! Content JP relays from elsewhere (echoed user messages, model output, tool
//! results) can carry control sequences that a terminal executes instead of
//! showing: cursor movement that rewrites a line the user already read, an
//! erase that hides one, an OSC that retitles the window or writes the
//! clipboard.
//! [`ContentWriter`] filters such content on its way to the terminal, keeping
//! what its [`ContentClass`] allows and rendering the rest the way the
//! [`SanitizeMode`] says.
//! [`visible_sgr`] is the writer's SGR parsing and conceal removal on their
//! own, for a filter that applies a policy of its own.
//! [`strip_controls`] removes control characters from text that is only ever
//! shown as plain text.
//!
//! Only what is displayed is filtered, never what is stored.
//! This is unrelated to `Workspace::sanitize` (storage) and
//! `ConversationStream::sanitize` (stream repair).

use std::fmt::{self, Write as _};

use vte::{Params, Parser, Perform};

use crate::ansi::RESET;

/// Shown in place of a dropped sequence under [`SanitizeMode::Visualize`].
const MARKER: char = '\u{241b}';

/// SGR conceal, which hides text from the reader.
const CONCEAL: u16 = 8;

/// CAN, which aborts a sequence in progress.
const CAN: u8 = 0x18;

/// SUB, which aborts a sequence in progress.
const SUB: u8 = 0x1a;

/// Where untrusted content comes from, which decides whether its styling
/// survives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentClass {
    /// Tool results and custom-formatter output.
    ///
    /// SGR styling is kept, so a colored diff stays colored, and
    /// [`ContentWriter::finish`] closes the content span with a reset.
    ToolOutput,

    /// LLM message and reasoning text.
    ///
    /// SGR is dropped along with every other sequence: markdown is how the
    /// model styles text.
    /// There is no content span, so [`ContentWriter::finish`] writes no reset.
    ModelOutput,
}

impl ContentClass {
    /// Whether SGR sequences survive the filter.
    const fn keeps_styling(self) -> bool {
        match self {
            Self::ToolOutput => true,
            Self::ModelOutput => false,
        }
    }

    /// Whether [`ContentWriter::finish`] closes a content span with a reset.
    const fn has_span(self) -> bool {
        match self {
            Self::ToolOutput => true,
            Self::ModelOutput => false,
        }
    }
}

/// What [`ContentWriter`] does with a sequence its class does not allow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SanitizeMode {
    /// Remove it.
    Strip,

    /// Replace it with a single visible `␛` (U+241B).
    Visualize,

    /// Pass all content through unchanged.
    ///
    /// A content span still closes with a reset.
    Off,
}

/// A writer that filters untrusted content down to what its [`ContentClass`]
/// allows.
///
/// Printable text, `\n`, and `\t` pass through.
/// SGR sequences pass when the class keeps styling, less any conceal parameter
/// (`SGR 8`), which would hide text from the reader.
/// Everything else is dropped: every other CSI sequence (cursor movement,
/// erasure, scrolling, mode changes), OSC, DCS, SOS, PM, and APC strings with
/// their payloads, the remaining escape sequences, the remaining C0 controls
/// including `\r`, DEL, and C1 controls.
/// [`SanitizeMode::Strip`] removes what is dropped, and
/// [`SanitizeMode::Visualize`] puts one `␛` in place of each dropped sequence,
/// dropped control, and removed conceal parameter.
/// Under [`SanitizeMode::Off`] the content passes through verbatim.
///
/// A [`vte::Parser`] recognizes the sequences, and its state persists across
/// `write_str` calls, so a sequence split between two writes is still seen
/// whole.
/// A write's output reaches the wrapped writer before the write returns; only
/// the bytes of a sequence still in progress are held back.
///
/// Constructing a writer for a class with a content span opens the span, and
/// [`finish`] closes it with `\x1b[0m` in every mode, so styling the content
/// opened ends with the content.
///
/// [`finish`]: Self::finish
pub struct ContentWriter<W: fmt::Write> {
    /// The wrapped writer that receives the filtered content.
    output: W,

    /// Recognizes escape sequences, holding a sequence split between writes
    /// until the rest of it arrives.
    parser: Parser,

    /// Applies the class's policy to what the parser recognizes.
    sink: Sink,
}

impl<W: fmt::Write> ContentWriter<W> {
    /// Wrap `output`, filtering everything written to it as `class` content
    /// under `mode`.
    #[must_use]
    pub fn new(output: W, class: ContentClass, mode: SanitizeMode) -> Self {
        Self {
            output,
            parser: Parser::new(),
            sink: Sink {
                class,
                mode,
                buffer: String::new(),
                open: false,
                st_pending: false,
            },
        }
    }

    /// End the content.
    ///
    /// A sequence the content left unfinished counts as dropped: removed, or
    /// marked with `␛` under [`SanitizeMode::Visualize`].
    /// For a class with a content span, `\x1b[0m` follows.
    ///
    /// The parser starts over, so content written afterwards is filtered on its
    /// own, without the unfinished sequence absorbing it, and a class with a
    /// span opens a new one.
    ///
    /// # Errors
    ///
    /// Propagates any error from the wrapped writer.
    pub fn finish(&mut self) -> fmt::Result {
        if self.sink.mode != SanitizeMode::Off {
            self.sink.settle();
            self.parser = Parser::new();
        }

        if self.sink.class.has_span() {
            self.sink.buffer.push_str(RESET);
        }

        self.flush()
    }

    /// Feed one `ESC` to the parser, recording that a sequence starts there.
    ///
    /// vte reports no start of a sequence, and no end of one it never
    /// dispatches: a CSI cut short by the next `ESC`, a malformed CSI, an SOS,
    /// PM, or APC string.
    /// A sequence still open when the next `ESC` arrives, and not settled by a
    /// callback while that `ESC` is parsed, ended there unreported.
    fn begin_escape(&mut self) {
        let was_open = self.sink.open;
        self.parser.advance(&mut self.sink, b"\x1b");

        if was_open && self.sink.open {
            self.sink.drop_sequence();
            // An SOS, PM, or APC string ends this way, and this `ESC` may be the
            // first half of its ST.
            self.sink.st_pending = true;
        }

        self.sink.open = true;
    }

    /// Hand what the current write produced to the wrapped writer.
    fn flush(&mut self) -> fmt::Result {
        if self.sink.buffer.is_empty() {
            return Ok(());
        }

        let result = self.output.write_str(&self.sink.buffer);
        self.sink.buffer.clear();
        result
    }
}

impl<W: fmt::Write> fmt::Write for ContentWriter<W> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if self.sink.mode == SanitizeMode::Off {
            return self.output.write_str(s);
        }

        let mut rest = s;
        while !rest.is_empty() {
            if let Some(after) = rest.strip_prefix('\x1b') {
                self.begin_escape();
                rest = after;
                continue;
            }

            let end = rest.find('\x1b').unwrap_or(rest.len());
            self.parser.advance(&mut self.sink, &rest.as_bytes()[..end]);
            rest = &rest[end..];
        }

        self.flush()
    }
}

/// Applies a content class's policy to what the parser recognizes.
///
/// vte reports a sequence when it ends and says nothing when one starts, so the
/// writer marks each `ESC` it feeds as the start of one, and the sink resolves
/// it on the callback that ends it.
struct Sink {
    /// Decides whether styling survives.
    class: ContentClass,

    /// Decides what a dropped sequence leaves behind.
    mode: SanitizeMode,

    /// The filtered output of the write in progress.
    ///
    /// vte reports text one character at a time; collecting a write's output
    /// hands it to the wrapped writer in one piece.
    buffer: String,

    /// Whether a sequence has started and not been resolved yet.
    open: bool,

    /// Whether the sequence just dropped was a string whose terminator is still
    /// arriving: the `\` of an ST whose `ESC` ended the string, or the CAN or
    /// SUB that did.
    ///
    /// That terminator belongs to the dropped string and earns no marker of its
    /// own.
    st_pending: bool,
}

impl Sink {
    /// Record something dropped: nothing under `strip`, a marker under
    /// `visualize`.
    fn mark(&mut self) {
        if self.mode == SanitizeMode::Visualize {
            self.buffer.push(MARKER);
        }
    }

    /// Resolve the open sequence as dropped.
    fn drop_sequence(&mut self) {
        self.open = false;
        self.st_pending = false;
        self.mark();
    }

    /// Resolve what the end of the content left open.
    ///
    /// An open sequence is dropped, unless it is the unfinished ST of a string
    /// already dropped.
    fn settle(&mut self) {
        if self.open && !self.st_pending {
            self.mark();
        }

        self.open = false;
        self.st_pending = false;
    }
}

impl Perform for Sink {
    fn print(&mut self, c: char) {
        // vte prints only from its ground state, so a sequence still open ended
        // without a dispatch: a malformed CSI.
        if self.open {
            self.drop_sequence();
        }
        self.st_pending = false;

        // vte prints DEL rather than executing it.
        if c.is_control() {
            self.mark();
        } else {
            self.buffer.push(c);
        }
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' | b'\t' => {
                self.st_pending = false;
                self.buffer.push(char::from(byte));
            }
            // CAN and SUB abort the sequence in progress, and one marker covers
            // both.
            CAN | SUB if self.open => self.drop_sequence(),
            // Or they end a string that was dropped a moment ago.
            CAN | SUB if self.st_pending => self.st_pending = false,
            _ => {
                self.st_pending = false;
                self.mark();
            }
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        if !is_sgr(intermediates, ignore, action) || !self.class.keeps_styling() {
            self.drop_sequence();
            return;
        }

        self.open = false;
        self.st_pending = false;

        let (kept, concealed) = without_conceal(params);
        if concealed {
            self.mark();
        }
        if !kept.is_empty() {
            push_sgr(&mut self.buffer, &kept);
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        // The `\` completing the ST of a string dropped a moment ago.
        if self.st_pending && byte == b'\\' && intermediates.is_empty() {
            self.open = false;
            self.st_pending = false;
            return;
        }

        self.drop_sequence();
    }

    fn osc_dispatch(&mut self, _params: &[&[u8]], bell_terminated: bool) {
        self.drop_sequence();
        self.st_pending = !bell_terminated;
    }

    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, _action: char) {
        // The payload that follows reaches `put`, which discards it.
        self.drop_sequence();
    }

    fn unhook(&mut self) {
        self.st_pending = true;
    }
}

/// The SGR sequence `escape` holds with conceal removed, or `None` when
/// `escape` is not SGR or held nothing but conceal.
///
/// `escape` is parsed the way a terminal parses it: the private marker in
/// `\x1b[>4;2m` makes it a keyboard-mode setting instead of SGR, and `\x1b[08m`
/// is conceal.
/// The sequence is rebuilt from its parsed parameters, so `\x1b[m` comes back
/// as `\x1b[0m`, and `\x1b[01m` as `\x1b[1m`.
#[must_use]
pub fn visible_sgr(escape: &str) -> Option<String> {
    let mut probe = SgrProbe::default();
    Parser::new().advance(&mut probe, escape.as_bytes());
    probe.visible
}

/// `text` with every control character removed except those in `keep`.
///
/// Control characters are C0 (line feeds and tabs among them), DEL, and C1.
/// Escape sequences are not parsed: the `ESC` that opens one is removed, and
/// the rest of it stays as visible text, so `\x1b[2J` becomes `[2J`.
#[must_use]
pub fn strip_controls(text: &str, keep: &[char]) -> String {
    text.chars()
        .filter(|c| !c.is_control() || keep.contains(c))
        .collect()
}

/// Keeps the visible part of the SGR sequence a parse dispatches.
#[derive(Default)]
struct SgrProbe {
    /// The sequence, rebuilt with conceal removed.
    visible: Option<String>,
}

impl Perform for SgrProbe {
    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        if !is_sgr(intermediates, ignore, action) {
            return;
        }

        let (kept, _concealed) = without_conceal(params);
        if kept.is_empty() {
            return;
        }

        let mut escape = String::new();
        push_sgr(&mut escape, &kept);
        self.visible = Some(escape);
    }
}

/// Whether a CSI dispatch is SGR.
///
/// A private marker or an intermediate byte turns a sequence ending in `m` into
/// something else, and a sequence with more parameters than vte holds was cut
/// short.
fn is_sgr(intermediates: &[u8], ignore: bool, action: char) -> bool {
    action == 'm' && intermediates.is_empty() && !ignore
}

/// An SGR sequence's parameter groups with conceal removed, and whether any
/// was.
///
/// Parameters are walked rather than filtered, because `38`, `48`, and `58`
/// carry their color in the parameters that follow them (`5;n` for an indexed
/// color, `2;r;g;b` for an RGB one), and one of those that happens to be `8` is
/// a color value, not conceal.
/// A color in the colon form (`38:5:8`) is a single group, kept whole.
fn without_conceal(params: &Params) -> (Vec<&[u16]>, bool) {
    let mut kept = Vec::new();
    let mut concealed = false;
    let mut groups = params.iter();

    while let Some(group) = groups.next() {
        match group {
            [38 | 48 | 58] => {
                kept.push(group);
                let Some(space) = groups.next() else { break };
                kept.push(space);

                let operands = match space.first() {
                    Some(5) => 1,
                    Some(2) => 3,
                    _ => 0,
                };
                kept.extend(groups.by_ref().take(operands));
            }
            // A terminal that ignores the sub-parameters of conceal still
            // conceals.
            [CONCEAL, ..] => concealed = true,
            _ => kept.push(group),
        }
    }

    (kept, concealed)
}

/// Append `groups` to `out` as an SGR sequence: groups separated by `;`, and
/// the sub-parameters within a group by `:`.
fn push_sgr(out: &mut String, groups: &[&[u16]]) {
    out.push_str("\x1b[");
    for (index, group) in groups.iter().enumerate() {
        if index > 0 {
            out.push(';');
        }
        for (sub_index, value) in group.iter().enumerate() {
            if sub_index > 0 {
                out.push(':');
            }
            // Writing to a `String` is infallible.
            let _ = write!(out, "{value}");
        }
    }
    out.push('m');
}

#[cfg(test)]
#[path = "sanitize_tests.rs"]
mod tests;
