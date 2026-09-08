//! A background colour applied to a region of terminal output.
//!
//! A region background is the fill behind a run of rows — the shading a
//! reasoning block sits in, the tint a status row is drawn against.
//! It is described by two things: the SGR parameter that sets the colour, and
//! how far along each row the colour extends.
//!
//! [`line_fill`] is the one place that second question is answered, so every
//! writer that maintains a background agrees on what a filled row looks like.

use std::borrow::Cow;

/// How a default background colour fills each line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundFill {
    /// Fill to the last visible character on the line.
    Content,

    /// Fill to a fixed column width (padding with spaces if needed).
    Column(usize),

    /// Fill to the end of the terminal window via `\x1b[K`.
    Terminal,
}

/// A default background colour applied to all content, with a fill mode
/// controlling how far it extends on each line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultBackground {
    /// SGR background parameter, e.g. `"48;5;236"` or `"48;2;80;73;69"`.
    pub param: String,

    /// How far the background extends on each line.
    pub fill: BackgroundFill,
}

/// The text that extends a background from `column` to the end of the line.
///
/// The one place [`BackgroundFill`] is interpreted.
/// Every writer that maintains a region background consults this so the three
/// modes cannot drift apart: `Content` adds nothing, `Terminal` defers to the
/// terminal's erase-to-end-of-line, and `Column` pads with real spaces — the
/// only form a host that lays out its own sub-window (an `fzf` preview pane)
/// renders, since it does not implement the erase.
///
/// The caller is responsible for having the background active before writing
/// the result.
#[must_use]
pub fn line_fill(fill: BackgroundFill, column: usize) -> Cow<'static, str> {
    match fill {
        BackgroundFill::Content => Cow::Borrowed(""),
        BackgroundFill::Terminal => Cow::Borrowed("\x1b[K"),
        BackgroundFill::Column(target) => match target.saturating_sub(column) {
            0 => Cow::Borrowed(""),
            pad => Cow::Owned(" ".repeat(pad)),
        },
    }
}
