//! Where the actions for a selection in the file show.
//!
//! A selection made with a mouse gets them where the mouse let go, clear of the
//! text selected: under the selection's last line after a drag forward, and
//! over its first line after a drag backward, at the pointer's place along the
//! line.
//!
//! A selection made without a pointer position, with a touch screen's handles
//! or the keyboard, gets them under the selection. iOS draws its own menu
//! (Copy, Look Up, Share) over a selection, which a page can't turn off, and
//! under it when there is no room above; the distances here keep clear of that
//! menu and of the handle hanging under the selection's end.
//! They are the ones Plannotator measured on iOS.
//!
//! Every position is in the coordinates `getBoundingClientRect` answers in.

/// A position on the page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// A box on the page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

/// What the actions are placed beside.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Anchor {
    /// A selection a mouse made: `x` is where along the line the mouse let go,
    /// `selection` the box around the lines selected, and `backward` says the
    /// mouse dragged from the selection's end to its start.
    Pointer {
        x: f64,
        backward: bool,
        selection: Rect,
    },

    /// The box around the lines of a selection made without a pointer.
    Selection(Rect),
}

/// The room kept between the actions and every edge of the visible page.
pub const MARGIN: f64 = 8.0;

/// The room kept between the actions and the lines a mouse selected.
pub const POINTER_GAP: f64 = 6.0;

/// The room kept under a selection, which clears the handle at its end and the
/// area around the handle that takes touches.
pub const HANDLE_GAP: f64 = 30.0;

/// The room kept over a selection.
pub const ABOVE_GAP: f64 = 8.0;

/// How far a selection's top is from the top of the visible page before iOS has
/// room to draw its menu over the selection.
pub const MENU_ROOM: f64 = 60.0;

/// The room iOS's menu takes, over or under a selection.
pub const MENU_CLEARANCE: f64 = 52.0;

/// The top left corner of actions `size` large, beside `anchor`, inside
/// `visible`, the part of the page the reader sees.
///
/// Actions larger than `visible` start at its top left corner.
#[must_use]
pub fn place(anchor: Anchor, size: Size, visible: Rect) -> Point {
    let min = Point {
        x: visible.left + MARGIN,
        y: visible.top + MARGIN,
    };
    let max = Point {
        x: (visible.right - MARGIN - size.width).max(min.x),
        y: (visible.bottom - MARGIN - size.height).max(min.y),
    };

    let (center, y) = match anchor {
        Anchor::Pointer {
            x,
            backward,
            selection,
        } => (
            x,
            beside_pointer(selection, backward, size.height, min.y, max.y),
        ),
        Anchor::Selection(selection) => (
            f64::midpoint(selection.left, selection.right),
            beside_selection(selection, size.height, min.y, max.y),
        ),
    };

    // Centered on `center`: the left edge is half the width back from it.
    let left = f64::midpoint(center, center - size.width);

    Point {
        x: left.clamp(min.x, max.x),
        y,
    }
}

/// The top of actions `height` tall beside the lines `selection` a mouse
/// selected: under them, or over them after a drag backward, the other side
/// when that one has no room between `min` and `max`, and as near the first as
/// fits when neither has.
fn beside_pointer(selection: Rect, backward: bool, height: f64, min: f64, max: f64) -> f64 {
    let under = selection.bottom + POINTER_GAP;
    let over = selection.top - POINTER_GAP - height;
    let (first, second) = if backward {
        (over, under)
    } else {
        (under, over)
    };

    [first, second]
        .into_iter()
        .find(|top| (min..=max).contains(top))
        .unwrap_or_else(|| first.clamp(min, max))
}

/// The top of actions `height` tall beside `selection`, between `min` and
/// `max`: under it; over it, above iOS's menu when that fits too, for a
/// selection with no room under it; and at `max` for one with room on neither
/// side.
fn beside_selection(selection: Rect, height: f64, min: f64, max: f64) -> f64 {
    // iOS draws its menu under a selection it has no room over.
    let menu_under = selection.top - min < MENU_ROOM;
    let under = [HANDLE_GAP + MENU_CLEARANCE, HANDLE_GAP]
        .into_iter()
        .skip(usize::from(!menu_under))
        .map(|gap| selection.bottom + gap)
        .find(|top| *top <= max);
    if let Some(top) = under {
        return top.max(min);
    }

    [ABOVE_GAP + MENU_CLEARANCE, ABOVE_GAP]
        .into_iter()
        .map(|gap| selection.top - gap - height)
        .find(|top| *top >= min)
        .map_or(max, |top| top.min(max))
}

#[cfg(test)]
#[path = "popover_tests.rs"]
mod tests;
