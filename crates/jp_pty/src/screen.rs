//! A snapshot of what a terminal is showing.

use std::fmt;

use crate::terminal::Size;

/// Every visible row of a terminal, the cursor, and which rows wrapped.
///
/// Owned rather than borrowed from the model, so a snapshot taken the moment a
/// wait was satisfied is the one the assertions after it are made against.
/// Trailing blanks are trimmed from each row, so a row reads as its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screen {
    /// The visible rows, top first.
    rows: Vec<String>,

    /// Per row, whether it continues onto the row below.
    wrapped: Vec<bool>,

    /// The cursor's `(row, column)`, zero-based.
    cursor: (u16, u16),

    /// The terminal's size when the snapshot was taken.
    size: Size,
}

impl Screen {
    /// Read the visible state out of a screen model.
    pub(crate) fn capture(screen: &vt100::Screen) -> Self {
        let (rows, columns) = screen.size();

        Self {
            rows: screen
                .rows(0, columns)
                .map(|row| row.trim_end().to_owned())
                .collect(),
            wrapped: (0..rows).map(|row| screen.row_wrapped(row)).collect(),
            cursor: screen.cursor_position(),
            size: Size::new(rows, columns),
        }
    }

    /// Every visible row, top first.
    #[must_use]
    pub fn rows(&self) -> &[String] {
        &self.rows
    }

    /// Row `index`, or `""` when the screen has no such row.
    ///
    /// A predicate that names a row the screen does not have never matches,
    /// which surfaces as a wait timing out with the screen attached.
    #[must_use]
    pub fn row(&self, index: usize) -> &str {
        self.rows.get(index).map_or("", String::as_str)
    }

    /// The last `count` visible rows.
    ///
    /// For a block anchored to the bottom of the screen.
    #[must_use]
    pub fn tail(&self, count: usize) -> &[String] {
        &self.rows[self.rows.len().saturating_sub(count)..]
    }

    /// Every row up to and including the last one with anything on it.
    ///
    /// For a block with blank screen below it, where [`Self::tail`] would
    /// return the empty rows underneath.
    #[must_use]
    pub fn used(&self) -> &[String] {
        let end = self
            .rows
            .iter()
            .rposition(|row| !row.is_empty())
            .map_or(0, |index| index + 1);

        &self.rows[..end]
    }

    /// The cursor's `(row, column)`, zero-based.
    #[must_use]
    pub const fn cursor(&self) -> (u16, u16) {
        self.cursor
    }

    /// Whether row `index` continues onto the row below it.
    ///
    /// A wrapped row occupies two physical rows, which is one more than code
    /// that counts the rows it drew expects.
    #[must_use]
    pub fn wrapped(&self, index: u16) -> bool {
        self.wrapped
            .get(usize::from(index))
            .copied()
            .unwrap_or(false)
    }

    /// The terminal's size when the snapshot was taken.
    #[must_use]
    pub const fn size(&self) -> Size {
        self.size
    }

    /// Whether any row contains `text`.
    ///
    /// Loose by design, for a predicate that waits for something to appear
    /// before the exact assertions run.
    #[must_use]
    pub fn contains(&self, text: &str) -> bool {
        self.rows.iter().any(|row| row.contains(text))
    }
}

impl fmt::Display for Screen {
    /// Render the screen the way a failure wants to read it: bordered, with the
    /// cursor's row marked.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let width = usize::from(self.size.columns);
        let rule = "─".repeat(width);

        writeln!(
            f,
            "   ┌{rule}┐  {}x{}, cursor at row {} column {}",
            self.size.rows, self.size.columns, self.cursor.0, self.cursor.1
        )?;
        for (index, row) in self.rows.iter().enumerate() {
            let mark = if u16::try_from(index) == Ok(self.cursor.0) {
                '>'
            } else {
                ' '
            };
            writeln!(f, "{index:>2}{mark}│{row:<width$}│")?;
        }
        write!(f, "   └{rule}┘")
    }
}

#[cfg(test)]
#[path = "screen_tests.rs"]
mod tests;
