//! Selection and scroll state for the dashboard table.
//!
//! [`Cursor`] tracks the selected item by its stable [`Key`] so the selection
//! follows the item across refreshes as rows are added, removed, or reordered.

use std::collections::HashMap;

use super::table::{Display, Item, Key};
use crate::model::{Row, RowId};

#[derive(Debug, Default)]
pub struct Cursor {
    pub(super) selected: Option<Key>,

    /// Worktree row to select as soon as it appears in the table.
    pub(super) wanted: Option<RowId>,

    /// Inner selection saved when collapsing a row with `Space`, restored on
    /// re-expand.
    pub(super) last_selected: HashMap<RowId, Key>,

    /// Index of the first visible display line.
    pub scroll: usize,

    /// Number of visible table body rows from the last [`Self::fit`] call.
    page: usize,
}

impl Cursor {
    /// Return the position of the selected item in `items`.
    #[must_use]
    pub fn index(&self, items: &[Item<'_>]) -> Option<usize> {
        let selected = self.selected.as_ref()?;
        items.iter().position(|item| item.key() == *selected)
    }

    /// Return the selected item if present in `items`.
    #[must_use]
    pub fn item<'a>(&self, items: &[Item<'a>]) -> Option<Item<'a>> {
        let selected = self.selected.as_ref()?;
        items.iter().find(|item| item.key() == *selected).copied()
    }

    /// Select the item at `index` in `items`.
    pub fn select(&mut self, items: &[Item<'_>], index: usize) {
        if let Some(item) = items.get(index) {
            self.selected = Some(item.key());
        }
    }

    /// Select the last item in `items`.
    pub fn select_last(&mut self, items: &[Item<'_>]) {
        self.select(items, items.len().saturating_sub(1));
    }

    /// Move the selection down or up by one item, wrapping around at the ends.
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

    /// Move the selection down or up by half a page, clamping at the ends.
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

    /// Move the selection down or up by one item, clamping at the ends.
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

    /// Adjust `scroll` so the selected item is visible within `rows` screen
    /// lines without trailing blank lines.
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

    /// Select `wanted` once a matching row appears in `rows`.
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

    /// Ensure the selection points to a visible item in `items`.
    ///
    /// If the selected child item disappeared, falls back to its parent row.
    /// If a pull request row was replaced by a newly created worktree row for
    /// that PR, follows the selection to the worktree row.
    /// Otherwise, selects the first item.
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
