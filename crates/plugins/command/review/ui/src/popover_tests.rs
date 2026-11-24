use pretty_assertions::assert_eq;

use super::*;

/// A desktop window.
const WINDOW: Rect = Rect {
    left: 0.0,
    top: 0.0,
    right: 1440.0,
    bottom: 900.0,
};

/// The two buttons a mouse is offered.
const BUTTONS: Size = Size {
    width: 140.0,
    height: 34.0,
};

/// Three lines of a paragraph, selected from the middle of the first to the
/// middle of the last.
const LINES: Rect = Rect {
    left: 280.0,
    top: 400.0,
    right: 1016.0,
    bottom: 475.0,
};

/// A phone's screen.
const PHONE: Rect = Rect {
    left: 0.0,
    top: 0.0,
    right: 390.0,
    bottom: 740.0,
};

/// The two buttons a finger is offered.
const TOUCH: Size = Size {
    width: 224.0,
    height: 54.0,
};

/// [`LINES`], as a mouse that let go at `x` selected it.
fn released(x: f64, backward: bool) -> Anchor {
    Anchor::Pointer {
        x,
        backward,
        selection: LINES,
    }
}

fn selected(top: f64, bottom: f64) -> Anchor {
    Anchor::Selection(Rect {
        left: 40.0,
        top,
        right: 300.0,
        bottom,
    })
}

/// Under the last line selected, where the mouse is, and clear of that line.
#[test]
fn a_drag_forward_gets_the_actions_under_the_line_it_ended_on() {
    let corner = place(released(600.0, false), BUTTONS, WINDOW);

    assert_eq!(corner, Point { x: 530.0, y: 481.0 });
}

/// The mouse is on the first line selected, and the rest lies under it.
#[test]
fn a_drag_backward_gets_the_actions_over_the_line_it_ended_on() {
    let corner = place(released(600.0, true), BUTTONS, WINDOW);

    assert_eq!(corner, Point { x: 530.0, y: 360.0 });
}

#[test]
fn a_drag_ending_at_the_bottom_of_the_window_gets_the_actions_over_the_selection() {
    let low = Anchor::Pointer {
        x: 600.0,
        backward: false,
        selection: Rect {
            top: 820.0,
            bottom: 890.0,
            ..LINES
        },
    };

    let corner = place(low, BUTTONS, WINDOW);

    assert_eq!(corner, Point { x: 530.0, y: 780.0 });
}

#[test]
fn a_drag_backward_ending_at_the_top_of_the_window_gets_the_actions_under_the_selection() {
    let high = Anchor::Pointer {
        x: 600.0,
        backward: true,
        selection: Rect {
            top: 20.0,
            bottom: 95.0,
            ..LINES
        },
    };

    let corner = place(high, BUTTONS, WINDOW);

    assert_eq!(corner, Point { x: 530.0, y: 101.0 });
}

#[test]
fn the_actions_stay_inside_the_sides_of_the_window() {
    let right = place(released(1430.0, false), BUTTONS, WINDOW);
    let left = place(released(10.0, false), BUTTONS, WINDOW);

    assert_eq!(right, Point {
        x: 1292.0,
        y: 481.0
    });
    assert_eq!(left, Point { x: 8.0, y: 481.0 });
}

/// A selection taller than the window has room on neither side of it, and keeps
/// the actions at the edge of the window the mouse is at.
#[test]
fn a_selection_taller_than_the_window_keeps_the_actions_inside_it() {
    let tall = Rect {
        top: -200.0,
        bottom: 1200.0,
        ..LINES
    };
    let forward = Anchor::Pointer {
        x: 600.0,
        backward: false,
        selection: tall,
    };
    let backward = Anchor::Pointer {
        x: 600.0,
        backward: true,
        selection: tall,
    };

    assert_eq!(place(forward, BUTTONS, WINDOW), Point {
        x: 530.0,
        y: 858.0
    });
    assert_eq!(place(backward, BUTTONS, WINDOW), Point { x: 530.0, y: 8.0 });
}

#[test]
fn a_selection_without_a_pointer_gets_the_actions_under_its_handle() {
    let corner = place(selected(300.0, 348.0), TOUCH, PHONE);

    assert_eq!(corner, Point { x: 58.0, y: 378.0 });
}

/// iOS has no room for its menu over a selection at the top of the screen, and
/// draws it under: the actions go under that.
#[test]
fn a_selection_at_the_top_of_the_screen_leaves_room_for_the_menu_under_it() {
    let corner = place(selected(20.0, 44.0), TOUCH, PHONE);

    assert_eq!(corner, Point { x: 58.0, y: 126.0 });
}

/// Over the selection, and over the menu iOS draws there.
#[test]
fn a_selection_at_the_bottom_of_the_screen_gets_the_actions_over_the_menu() {
    let corner = place(selected(640.0, 700.0), TOUCH, PHONE);

    assert_eq!(corner, Point { x: 58.0, y: 526.0 });
}

/// A selection with room over it for the actions and not for the menu too gets
/// the actions right over it.
#[test]
fn a_selection_with_little_room_over_it_gets_the_actions_right_over_it() {
    let short = Rect {
        bottom: 200.0,
        ..PHONE
    };

    let corner = place(selected(100.0, 180.0), TOUCH, short);

    assert_eq!(corner, Point { x: 58.0, y: 38.0 });
}

#[test]
fn a_selection_taller_than_the_screen_gets_the_actions_at_its_bottom() {
    let corner = place(selected(-100.0, 900.0), TOUCH, PHONE);

    assert_eq!(corner, Point { x: 58.0, y: 678.0 });
}

#[test]
fn a_selection_scrolled_over_the_top_keeps_the_actions_on_the_screen() {
    let corner = place(selected(-200.0, -100.0), TOUCH, PHONE);

    assert_eq!(corner, Point { x: 58.0, y: 8.0 });
}

/// The on-screen keyboard and a pinch zoom move the part of the page seen.
#[test]
fn the_actions_stay_inside_the_part_of_the_page_seen() {
    let zoomed = Rect {
        left: 100.0,
        top: 200.0,
        right: 295.0,
        bottom: 570.0,
    };

    let corner = place(selected(300.0, 348.0), TOUCH, zoomed);

    assert_eq!(corner, Point { x: 108.0, y: 378.0 });
}
