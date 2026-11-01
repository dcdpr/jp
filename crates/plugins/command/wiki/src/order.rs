//! The order pages are read in: the top pages, then down each page's children.
//!
//! A page's parent lives in its own file; the order among siblings belongs to
//! no single page, so it lives in one file next to them, [`ORDER_FILE`]:
//!
//! ```json
//! {
//!   "top": ["W-0a1b2c3"],
//!   "children": { "W-0a1b2c3": ["W-0a1b2c4"] }
//! }
//! ```
//!
//! A page the file doesn't name sorts after the ones it does, by id.
//! [`layout`] lays out the active pages as the sidebar shows them, and
//! [`Order::rank`] moves one among its siblings.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    iter,
};

use camino::Utf8Path;
use serde::{Deserialize, Serialize};

use crate::page::{Page, PageId};

/// The order file, inside the wiki directory.
pub(crate) const ORDER_FILE: &str = ".order.json";

/// Where a page sits among its siblings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Place {
    /// A top page, with no parent.
    Top,

    /// Under a parent page.
    Children(PageId),
}

/// The ranked pages of each sibling list, top first, as their ids are written.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Order {
    #[serde(default)]
    pub top: Vec<String>,

    #[serde(default)]
    pub children: BTreeMap<String, Vec<String>>,
}

impl Order {
    /// Read the order from the wiki directory.
    ///
    /// A wiki without an order file has ranked nothing yet.
    /// A file that is there but doesn't hold an order is an error, so a write
    /// never replaces what it couldn't read.
    pub(crate) fn read(dir: &Utf8Path) -> Result<Self, String> {
        let path = dir.join(ORDER_FILE);
        let source = match std::fs::read_to_string(&path) {
            Ok(source) => source,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(format!("{path}: {error}")),
        };

        if source.trim().is_empty() {
            return Ok(Self::default());
        }

        serde_json::from_str(&source).map_err(|error| format!("{path} is not an order: {error}."))
    }

    /// Write the order into the wiki directory, two-space indented with a
    /// trailing newline, dropping children lists left empty.
    pub(crate) fn write(&self, dir: &Utf8Path) -> Result<(), String> {
        let mut order = self.clone();
        order.children.retain(|_, ids| !ids.is_empty());

        let path = dir.join(ORDER_FILE);
        let mut json =
            serde_json::to_string_pretty(&order).map_err(|error| format!("{path}: {error}"))?;
        json.push('\n');

        std::fs::write(&path, json).map_err(|error| format!("{path}: {error}"))
    }

    /// The ranked entries of the sibling list at `place`.
    #[must_use]
    pub(crate) fn list(&self, place: Place) -> &[String] {
        match place {
            Place::Top => &self.top,
            Place::Children(parent) => self
                .children
                .get(&parent.to_string())
                .map_or(&[], Vec::as_slice),
        }
    }

    fn list_mut(&mut self, place: Place) -> &mut Vec<String> {
        match place {
            Place::Top => &mut self.top,
            Place::Children(parent) => self.children.entry(parent.to_string()).or_default(),
        }
    }

    /// Move `id` directly before or after `other`, both among `siblings` at
    /// `place`.
    ///
    /// The list is written out down to the lowest page that has to be ranked to
    /// hold its place: the moved one, or one ranked already.
    /// Pages below that stay unranked and keep sorting by id.
    ///
    /// Returns `false`, and changes nothing, when either page is missing from
    /// `siblings` or they are the same page.
    pub(crate) fn rank(
        &mut self,
        place: Place,
        siblings: &[PageId],
        id: PageId,
        before: bool,
        other: PageId,
    ) -> bool {
        if id == other || !siblings.contains(&id) || !siblings.contains(&other) {
            return false;
        }

        let ranked: HashSet<PageId> = self
            .list(place)
            .iter()
            .filter_map(|entry| entry.parse().ok())
            .collect();

        let mut laid_out = arrange(self.list(place), siblings);
        laid_out.retain(|page| *page != id);
        let Some(anchor) = laid_out.iter().position(|page| *page == other) else {
            return false;
        };
        laid_out.insert(if before { anchor } else { anchor + 1 }, id);

        let keep = laid_out
            .iter()
            .rposition(|page| *page == id || ranked.contains(page))
            .map_or(0, |last| last + 1);
        *self.list_mut(place) = laid_out[..keep].iter().map(ToString::to_string).collect();

        true
    }

    /// Drop `id` from every list, and the list of its children, returning
    /// whether anything changed.
    pub(crate) fn remove(&mut self, id: PageId) -> bool {
        let children = self.children.remove(&id.to_string()).is_some();

        self.unrank(id) || children
    }

    /// Drop `id` from every list it is ranked in, keeping the order of its
    /// children, returning whether anything changed.
    ///
    /// A page that moves to another parent arrives there unranked.
    pub(crate) fn unrank(&mut self, id: PageId) -> bool {
        let mut changed = false;
        for list in iter::once(&mut self.top).chain(self.children.values_mut()) {
            let before = list.len();
            list.retain(|entry| entry.parse::<PageId>().ok() != Some(id));
            changed |= list.len() != before;
        }

        changed
    }

    /// Every entry, with the list it is in: `top` or `children.W-0a1b2c3`, and
    /// each key of `children` itself.
    #[must_use]
    pub(crate) fn entries(&self) -> Vec<(String, String)> {
        let mut entries: Vec<(String, String)> = self
            .top
            .iter()
            .map(|id| ("top".to_owned(), id.clone()))
            .collect();
        for (parent, ids) in &self.children {
            entries.push(("children".to_owned(), parent.clone()));
            entries.extend(
                ids.iter()
                    .map(|id| (format!("children.{parent}"), id.clone())),
            );
        }

        entries
    }
}

/// `siblings` in the order a list ranks them: the ranked ones first, in its
/// order, then the rest by id.
#[must_use]
pub(crate) fn arrange(ranked: &[String], siblings: &[PageId]) -> Vec<PageId> {
    // Collected rather than searched per comparison. A page named twice takes
    // its last position.
    let ranks: HashMap<PageId, usize> = ranked
        .iter()
        .enumerate()
        .filter_map(|(rank, entry)| Some((entry.parse().ok()?, rank)))
        .collect();

    let mut laid_out = siblings.to_vec();
    laid_out.sort_by_key(|id| (ranks.get(id).copied().unwrap_or(usize::MAX), *id));

    laid_out
}

/// The parent a page sits under: the active page its `Parent` names, unless
/// that is the page itself.
fn parent_of(page: &Page, active: &HashSet<PageId>) -> Option<PageId> {
    page.parent_id()
        .filter(|parent| *parent != page.id && active.contains(parent))
}

/// Where `page` sits among its siblings: under its parent when it has an active
/// one, otherwise at the top.
#[must_use]
pub(crate) fn place_of(page: &Page, pages: &[Page]) -> Place {
    let active: HashSet<PageId> = pages
        .iter()
        .filter(|page| !page.is_archived())
        .map(|page| page.id)
        .collect();

    parent_of(page, &active).map_or(Place::Top, Place::Children)
}

/// The active pages at `place`, by id.
#[must_use]
pub(crate) fn siblings(place: Place, pages: &[Page]) -> Vec<PageId> {
    let mut ids: Vec<PageId> = pages
        .iter()
        .filter(|page| !page.is_archived() && place_of(page, pages) == place)
        .map(|page| page.id)
        .collect();
    ids.sort_unstable();
    ids.dedup();

    ids
}

/// The active pages in reading order, each with its depth below the top: each
/// top page, then down its children.
///
/// A page whose parents loop back to it is laid out at the top once the rest
/// are placed, so every page appears once.
#[must_use]
pub(crate) fn layout<'a>(pages: &'a [Page], order: &Order) -> Vec<(usize, &'a Page)> {
    let active: Vec<&Page> = pages.iter().filter(|page| !page.is_archived()).collect();
    let ids: HashSet<PageId> = active.iter().map(|page| page.id).collect();
    let by_id: HashMap<PageId, &Page> = active.iter().map(|page| (page.id, *page)).collect();

    let mut out = vec![];
    let mut placed = HashSet::new();
    let tops: Vec<PageId> = active
        .iter()
        .filter(|page| parent_of(page, &ids).is_none())
        .map(|page| page.id)
        .collect();
    for id in arrange(order.list(Place::Top), &tops) {
        descend(id, 0, &active, &ids, &by_id, order, &mut placed, &mut out);
    }

    // Pages whose parents loop, which no top page reaches.
    let mut stranded: Vec<PageId> = ids.difference(&placed).copied().collect();
    stranded.sort_unstable();
    for id in stranded {
        descend(id, 0, &active, &ids, &by_id, order, &mut placed, &mut out);
    }

    out
}

/// Lay out `id` at `depth`, then its children below it.
fn descend<'a>(
    id: PageId,
    depth: usize,
    active: &[&'a Page],
    ids: &HashSet<PageId>,
    by_id: &HashMap<PageId, &'a Page>,
    order: &Order,
    placed: &mut HashSet<PageId>,
    out: &mut Vec<(usize, &'a Page)>,
) {
    if !placed.insert(id) {
        return;
    }
    let Some(page) = by_id.get(&id) else {
        return;
    };
    out.push((depth, *page));

    let children: Vec<PageId> = active
        .iter()
        .filter(|child| parent_of(child, ids) == Some(id))
        .map(|child| child.id)
        .collect();
    for child in arrange(order.list(Place::Children(id)), &children) {
        descend(child, depth + 1, active, ids, by_id, order, placed, out);
    }
}

#[cfg(test)]
#[path = "order_tests.rs"]
mod tests;
