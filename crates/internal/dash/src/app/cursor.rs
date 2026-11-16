//! The selected line of the table, and how far the table is scrolled.
//!
//! [`Cursor`] remembers the selection by its item's [`Key`], so it stays on its
//! row or child as lines come and go; every method takes the lines it works on
//! from the table.

use std::collections::HashMap;

use super::table::{Display, Item, Key};
use crate::model::{Row, RowId};

#[derive(Debug, Default)]
pub struct Cursor {
    /// The selected line.
    pub(super) selected: Option<Key>,

    /// A row to select as soon as it shows up: a worktree being created.
    pub(super) wanted: Option<RowId>,

    /// Selected item inside a row when it was collapsed with `Space`, restored
    /// on re-expand.
    pub(super) last_selected: HashMap<RowId, Key>,

    /// The first drawn line on screen.
    /// See [`Self::fit`].
    pub scroll: usize,

    /// The number of table lines on screen, as of the last [`Self::fit`].
    page: usize,
}

impl Cursor {
    /// The position of the selected line in `items`.
    #[must_use]
    pub fn index(&self, items: &[Item<'_>]) -> Option<usize> {
        let selected = self.selected.as_ref()?;
        items.iter().position(|item| item.key() == *selected)
    }

    /// The selected line, when it is among `items`.
    #[must_use]
    pub fn item<'a>(&self, items: &[Item<'a>]) -> Option<Item<'a>> {
        let selected = self.selected.as_ref()?;
        items.iter().find(|item| item.key() == *selected).copied()
    }

    /// Select the line at `index` in `items`, when there is one.
    pub fn select(&mut self, items: &[Item<'_>], index: usize) {
        if let Some(item) = items.get(index) {
            self.selected = Some(item.key());
        }
    }

    /// Select the last of `items`.
    pub fn select_last(&mut self, items: &[Item<'_>]) {
        self.select(items, items.len().saturating_sub(1));
    }

    /// Move the selection one line down (or up), wrapping from the last line to
    /// the first and back.
    pub fn step(&mut self, items: &[Item<'_>], down: bool) {
        let Some(last) = items.len().checked_sub(1) else {
            return;
        };

        let index = match self.index(items) {
            None => 0,
            Some(index) if down => {
                if index == last {
                    0
                } else {
                    index + 1
                }
            }
            Some(index) => index.checked_sub(1).unwrap_or(last),
        };
        self.select(items, index);
    }

    /// Move the selection half a screen down (or up), stopping at either end.
    pub fn jump(&mut self, items: &[Item<'_>], down: bool) {
        let Some(last) = items.len().checked_sub(1) else {
            return;
        };

        let by = (self.page / 2).max(1);
        let index = self.index(items).map_or(0, |index| {
            if down {
                (index + by).min(last)
            } else {
                index.saturating_sub(by)
            }
        });
        self.select(items, index);
    }

    /// Move the selection one line down (or up) for a turn of the scroll wheel,
    /// stopping at either end rather than wrapping.
    pub fn wheel(&mut self, items: &[Item<'_>], down: bool) {
        let Some(last) = items.len().checked_sub(1) else {
            return;
        };

        let index = self.index(items).map_or(0, |index| {
            if down {
                (index + 1).min(last)
            } else {
                index.saturating_sub(1)
            }
        });
        self.select(items, index);
    }

    /// Scroll so the selected line of `display` is among the `rows` lines on
    /// screen, without leaving lines empty past the end of the table.
    pub fn fit(&mut self, display: &[Display<'_>], rows: usize) {
        self.page = rows;
        let line = self.selected.as_ref().and_then(|selected| {
            display.iter().position(
                |line| matches!(line, Display::Item { item, .. } if item.key() == *selected),
            )
        });

        if let Some(line) = line {
            if rows > 0 && line >= self.scroll + rows {
                self.scroll = line + 1 - rows;
            }
            if line < self.scroll {
                self.scroll = line;
            }
        }

        self.scroll = self.scroll.min(display.len().saturating_sub(rows));
    }

    /// Select the wanted row, once it is among `rows`.
    pub fn follow(&mut self, rows: &[&Row]) {
        let Some(wanted) = self.wanted.take() else {
            return;
        };

        if rows.iter().any(|row| row.id == wanted) {
            self.selected = Some(Key {
                row: wanted,
                child: None,
            });
        } else {
            self.wanted = Some(wanted);
        }
    }

    /// Repair the selection when its line is not among `items`.
    ///
    /// A child that is gone falls back to its row, and a pull request that got
    /// a worktree follows it to the worktree's row.
    /// Anything else falls back to the first line.
    pub fn keep_visible(&mut self, items: &[Item<'_>]) {
        if self.index(items).is_some() {
            return;
        }

        let fallback = self.selected.as_ref().and_then(|key| {
            items
                .iter()
                .find(|item| match (item, &key.row) {
                    (Item::Row(row), RowId::PullRequest(number)) => {
                        row.pr.as_ref().is_some_and(|pr| pr.number == *number)
                    }
                    (Item::Row(row), id) => row.id == *id,
                    _ => false,
                })
                .map(Item::key)
        });

        self.selected = fallback.or_else(|| items.first().map(Item::key));
    }
}
