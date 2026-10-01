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
//! [`sanitize_str`] runs the same filter over text that is already whole, and
//! [`sanitize_unclosed`] leaves closing a tool output's span to the caller.
//! [`sanitize_decoded`] filters text again after a markdown parser has turned
//! character references such as `&#27;` into the characters they name.
//! [`visible_sgr`] is the writer's SGR parsing and conceal removal on their
//! own, for a filter that applies a policy of its own.
//! [`strip_controls`] removes control characters from text that is only ever
//! shown as plain text.
//! [`OutputFloor`] is the least filtering any printed text gets: it keeps the
//! sequences JP's own output is made of, whoever wrote the text, so a path that
//! forgot to filter its content still cannot take over the terminal.
//!
//! Only what is displayed is filtered, never what is stored.
//! This is unrelated to `Workspace::sanitize` (storage) and
//! `ConversationStream::sanitize` (stream repair).

use std::{
    borrow::Cow,
    fmt::{self, Write as _},
};

use vte::{Params, Parser, Perform};

use crate::ansi::RESET;

/// Shown in place of a dropped sequence under [`SanitizeMode::Visualize`].
const MARKER: char = '\u{241b}';

/// SGR conceal, which hides text from the reader.
const CONCEAL: u16 = 8;

/// Erase from the cursor to the end of the line.
const ERASE_TO_END: &str = "\x1b[K";

/// The first parameter of an OSC 8 hyperlink.
const HYPERLINK: &[u8] = b"8";

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

    /// Messages the user wrote, echoed and replayed.
    ///
    /// SGR styling is kept, so a pasted colored log looks like the log.
    /// The content span closes on the message's formatted output rather than in
    /// [`ContentWriter::finish`], which writes no reset: a reset in the
    /// markdown source could change how the message parses.
    UserMessage,

    /// Strings JP lays out from conversation data: conversation titles, and the
    /// lines `jp conversation grep` shows.
    ///
    /// SGR is dropped: the text is fit to a column budget, and JP styles it
    /// itself.
    /// There is no content span, so [`ContentWriter::finish`] writes no reset.
    DerivedString,
}

impl ContentClass {
    /// What the class's content may keep.
    const fn policy(self) -> Policy {
        let styling = match self {
            Self::ToolOutput | Self::UserMessage => true,
            Self::ModelOutput | Self::DerivedString => false,
        };

        Policy {
            styling,
            span: matches!(self, Self::ToolOutput),
            erase_line: false,
            hyperlinks: false,
        }
    }
}

/// What a filter lets through, beyond printable text, `\n`, and `\t`.
#[derive(Debug, Clone, Copy)]
#[expect(clippy::struct_excessive_bools)]
struct Policy {
    /// SGR sequences, less conceal.
    styling: bool,

    /// Whether [`ContentWriter::finish`] closes a content span with a reset.
    span: bool,

    /// Erase to the end of the line (`\x1b[K`).
    erase_line: bool,

    /// OSC 8 hyperlinks, with control characters removed from their targets.
    hyperlinks: bool,
}

/// What [`OutputFloor`] lets through: the sequences JP's own output uses.
const FLOOR: Policy = Policy {
    styling: true,
    span: false,
    erase_line: true,
    hyperlinks: true,
};

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
/// For tool output, constructing a writer opens the content span, and
/// [`finish`] closes it with `\x1b[0m` in every mode, so styling the content
/// opened ends with the content.
/// A user message's span is closed by the caller, on the formatted message.
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
        Self::with_policy(output, class.policy(), mode)
    }

    /// Wrap `output`, filtering everything written to it down to `policy` under
    /// `mode`.
    fn with_policy(output: W, policy: Policy, mode: SanitizeMode) -> Self {
        Self {
            output,
            parser: Parser::new(),
            sink: Sink {
                policy,
                mode,
                buffer: String::new(),
                open: false,
                st_pending: false,
                styled: false,
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
        self.settle();

        if self.sink.policy.span {
            self.sink.buffer.push_str(RESET);
        }

        self.flush()
    }

    /// The writer being filtered into.
    ///
    /// A caller filtering into a buffer takes each write's output from here as
    /// it goes.
    /// A sequence a write cut short stays held for the next write.
    pub const fn get_mut(&mut self) -> &mut W {
        &mut self.output
    }

    /// Resolve a sequence the content left unfinished as dropped, and start the
    /// parser over.
    fn settle(&mut self) {
        if self.sink.mode == SanitizeMode::Off {
            return;
        }

        self.sink.settle();
        self.parser = Parser::new();
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

/// Applies a policy to what the parser recognizes.
///
/// vte reports a sequence when it ends and says nothing when one starts, so the
/// writer marks each `ESC` it feeds as the start of one, and the sink resolves
/// it on the callback that ends it.
struct Sink {
    /// Decides which sequences survive.
    policy: Policy,

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

    /// Whether styling let through since the last full reset may still be in
    /// effect.
    styled: bool,
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
        if self.policy.erase_line && is_erase_to_end(params, intermediates, ignore, action) {
            self.open = false;
            self.st_pending = false;
            self.buffer.push_str(ERASE_TO_END);
            return;
        }

        if !is_sgr(intermediates, ignore, action) || !self.policy.styling {
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
            // Counted as closed only by a sequence that does nothing but reset;
            // anything else may leave some attribute on.
            self.styled = kept.iter().any(|group| *group != [0]);
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

    fn osc_dispatch(&mut self, params: &[&[u8]], bell_terminated: bool) {
        if self.policy.hyperlinks && params.first() == Some(&HYPERLINK) {
            self.open = false;
            push_hyperlink(&mut self.buffer, params, bell_terminated);
        } else {
            self.drop_sequence();
        }

        // Ended by `ESC \`, the `\` is still to come, and belongs to this string.
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

/// `text`, whole, filtered as `class` content under `mode`.
///
/// A [`ContentWriter`] written once and finished: a sequence `text` leaves
/// unfinished is dropped, and tool output ends with the reset that closes its
/// span.
#[must_use]
pub fn sanitize_str(text: &str, class: ContentClass, mode: SanitizeMode) -> String {
    let mut writer = ContentWriter::new(String::new(), class, mode);

    // Writing to a `String` is infallible.
    let _ = writer.write_str(text);
    let _ = writer.finish();

    writer.output
}

/// `text`, whole, filtered as `class` content under `mode`, with no reset to
/// close its span.
///
/// For a caller that writes only part of `text` and closes the span after that
/// part: a tool result cut to its first lines ends its styling after them, not
/// at the end of the text it cut.
/// A sequence `text` leaves unfinished is dropped.
#[must_use]
pub fn sanitize_unclosed(text: &str, class: ContentClass, mode: SanitizeMode) -> String {
    let mut writer = ContentWriter::new(String::new(), class, mode);

    // Writing to a `String` is infallible.
    let _ = writer.write_str(text);
    writer.settle();
    let _ = writer.flush();

    writer.output
}

/// Text a markdown parser decoded from character references, filtered again as
/// `class` content under `mode`.
///
/// The parser turns a reference such as `&#27;` into the character it names
/// after the source has been through a [`ContentWriter`], so a control
/// character in decoded text is one the writer never saw.
///
/// - Model output and derived strings lose every control character except `\n`
///   and `\t`, each replaced by `␛` under [`SanitizeMode::Visualize`].
///   Its source has already lost every escape sequence, so the text after a
///   decoded `ESC` was never part of one: `&#27;[2J` shows as `[2J`.
/// - User messages and tool output are filtered by the allowlist again, which
///   keeps their styling.
///   No reset is added: a span closes where the content ends, which a single
///   piece of decoded text never is.
/// - Under [`SanitizeMode::Off`], and for text whose only control characters
///   are `\n` and `\t`, the text is returned as it is.
#[must_use]
pub fn sanitize_decoded(text: &str, class: ContentClass, mode: SanitizeMode) -> Cow<'_, str> {
    let filtered = |c: char| c.is_control() && !matches!(c, '\n' | '\t');
    if mode == SanitizeMode::Off || !text.chars().any(filtered) {
        return Cow::Borrowed(text);
    }

    if !class.policy().styling {
        let mut kept = String::with_capacity(text.len());
        for c in text.chars() {
            if !filtered(c) {
                kept.push(c);
            } else if mode == SanitizeMode::Visualize {
                kept.push(MARKER);
            }
        }

        return Cow::Owned(kept);
    }

    Cow::Owned(sanitize_unclosed(text, class, mode))
}

/// The least filtering any text bound for a terminal gets: the sequences JP's
/// own output is made of, and nothing else.
///
/// Printable text, `\n`, and `\t` pass, as do SGR styling less conceal, erase
/// to the end of the line (`\x1b[K`), and OSC 8 hyperlinks, whose targets lose
/// their control characters.
/// Every other sequence and control character is dropped the way the
/// [`SanitizeMode`] says, as [`ContentWriter`] drops it, `\r` included.
///
/// The floor keeps text from moving the cursor, clearing the screen or a line,
/// switching terminal modes, or reaching past the character grid to the window
/// title or the clipboard.
/// An erase to the end of the line can only paint the rest of the row in the
/// current background: without `\r` the cursor never moves back over text
/// already written.
/// It does not replace filtering content by its [`ContentClass`]: it lets
/// through styling that model output should not carry.
///
/// One floor filters one stream, so a sequence one call leaves unfinished is
/// completed or dropped by the next.
pub struct OutputFloor {
    /// Filters into a buffer each call empties.
    writer: ContentWriter<String>,
}

impl OutputFloor {
    /// A floor that drops what it does not allow the way `mode` says.
    #[must_use]
    pub fn new(mode: SanitizeMode) -> Self {
        Self {
            writer: ContentWriter::with_policy(String::new(), FLOOR, mode),
        }
    }

    /// Filter under `mode` from now on.
    ///
    /// A sequence still in progress is dropped first, under the mode it began
    /// in; a marker that leaves behind comes out of the next call.
    pub fn set_mode(&mut self, mode: SanitizeMode) {
        self.writer.settle();
        // Writing to a `String` is infallible.
        let _ = self.writer.flush();
        self.writer.sink.mode = mode;
    }

    /// `text` as a terminal may receive it.
    ///
    /// A sequence `text` ends in the middle of is held back, to be completed or
    /// dropped by what the next call brings.
    pub fn filter(&mut self, text: &str) -> String {
        // Writing to a `String` is infallible.
        let _ = self.writer.write_str(text);
        std::mem::take(&mut self.writer.output)
    }

    /// End the stream: drop a sequence left unfinished, and close with
    /// `\x1b[0m` any styling still in effect.
    ///
    /// Under [`SanitizeMode::Off`] text is not parsed, so nothing is dropped or
    /// closed.
    pub fn finish(&mut self) -> String {
        self.writer.settle();

        if self.writer.sink.styled {
            self.writer.sink.styled = false;
            self.writer.sink.buffer.push_str(RESET);
        }

        // Writing to a `String` is infallible.
        let _ = self.writer.flush();
        std::mem::take(&mut self.writer.output)
    }
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

/// Whether a CSI dispatch erases from the cursor to the end of the line:
/// `\x1b[K` or `\x1b[0K`.
///
/// Erasing the whole line or the part before the cursor is not one of them.
fn is_erase_to_end(params: &Params, intermediates: &[u8], ignore: bool, action: char) -> bool {
    action == 'K' && intermediates.is_empty() && !ignore && params.iter().all(|g| matches!(g, [0]))
}

/// Append the OSC 8 hyperlink `params` hold, ended the way it was, with every
/// control character removed from it.
fn push_hyperlink(out: &mut String, params: &[&[u8]], bell_terminated: bool) {
    out.push_str("\x1b]");
    for (index, param) in params.iter().enumerate() {
        if index > 0 {
            out.push(';');
        }
        out.push_str(&strip_controls(&String::from_utf8_lossy(param), &[]));
    }
    out.push_str(if bell_terminated { "\x07" } else { "\x1b\\" });
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
