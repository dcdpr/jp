import AppKit
import SwiftUI

/// Stops the table view under a SwiftUI `List` from drawing its own selection.
///
/// Only the drawing is suppressed. The selection is still the list's, so click
/// selection, the arrow keys and `contextMenu(forSelectionType:)` all keep
/// working, and the row draws the selection the design calls for.
///
/// This reaches for AppKit because SwiftUI offers no way to say it. A `List` on
/// macOS is an `NSTableView`, and a selected row is filled with the system accent
/// colour by the row view itself, underneath whatever the row draws. Neither
/// `listRowBackground` nor an opaque fill in the row's own content hides it.
///
/// Put it in a *row*, not behind the list:
///
/// ```swift
/// List(...) { item in
///     ItemRow(item)
///         .background(ListSelectionHighlight.removed)
/// }
/// ```
///
/// A row's backing view is a descendant of the table view, so it can walk up to
/// the table in two hops. A view placed behind the whole list cannot: it is built
/// before the table exists, and finds nothing to configure.
enum ListSelectionHighlight {
    /// A view that turns the highlight off for the table holding it.
    ///
    /// Draws nothing, and fills whatever it is given rather than being sized to
    /// nothing: SwiftUI builds no backing view for a subview with no area, and one
    /// that is never built never runs.
    static var removed: some View {
        Remover()
    }

    private struct Remover: NSViewRepresentable {
        func makeNSView(context: Context) -> NSView {
            Probe()
        }

        /// Applied again on every update, which is what makes this hold: rows are
        /// realized and recycled as the list scrolls, and a table view SwiftUI
        /// rebuilt is back to drawing its own selection until the next row asks it
        /// not to.
        func updateNSView(_ view: NSView, context: Context) {
            (view as? Probe)?.silenceSelection()
        }
    }

    /// A view that does nothing but reach the table view it sits inside.
    private final class Probe: NSView {
        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            silenceSelection()
        }

        /// Turn off the highlight on the table view above this one.
        ///
        /// Walks the ancestors and tests each, rather than searching their
        /// subtrees: from inside a row the table is two hops up, and searching a
        /// table's subtree means walking every row it has realized.
        ///
        /// Finds nothing when called before the row is in the hierarchy, which the
        /// first call after `makeNSView` always is. The call from
        /// `viewDidMoveToWindow` is the one that lands.
        func silenceSelection() {
            var ancestor = superview

            while let current = ancestor {
                if let table = current as? NSTableView {
                    table.selectionHighlightStyle = .none
                    return
                }
                ancestor = current.superview
            }
        }
    }
}
