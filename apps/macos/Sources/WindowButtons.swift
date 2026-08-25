import AppKit
import SwiftUI

/// Moves the close, minimize and zoom buttons down the window.
///
/// macOS centres them 14 points below the top edge, which is the middle of a
/// standard title bar. A window with no title bar and a taller control in that
/// corner — a search field, say — leaves them sitting above that control's centre
/// rather than level with it.
///
/// There is no supported way to ask for this. A title bar grows to fit a toolbar,
/// and a toolbar spans the whole window: it would put a strip of chrome above the
/// transcript, which is the thing having no title bar was for. So the buttons are
/// moved directly, and moved again whenever AppKit lays the title bar out afresh.
///
/// Add it as a background of whatever the buttons should line up with:
///
/// ```swift
/// SearchField(text: $query)
///     .background(WindowButtons.placed(leading: 18, centredOn: 24))
/// ```
enum WindowButtons {
    /// A view that puts the window buttons `leading` points from the window's left
    /// edge, centred `distance` points below its top.
    ///
    /// Draws nothing. Does nothing while `distance` is not a real measurement, so
    /// a caller measuring the control can pass what it has before the first
    /// layout without the buttons jumping to the top of the window.
    static func placed(leading: CGFloat, centredOn distance: CGFloat) -> some View {
        Mover(leading: leading, distance: distance)
    }

    /// How far apart the buttons sit, centre to centre.
    ///
    /// What macOS itself uses, kept because the spacing is not what is being
    /// changed here: measured off a running window, the three frames sit at 20
    /// point intervals.
    static let spacing: CGFloat = 20

    private struct Mover: NSViewRepresentable {
        let leading: CGFloat
        let distance: CGFloat

        func makeNSView(context: Context) -> NSView {
            Probe()
        }

        func updateNSView(_ view: NSView, context: Context) {
            guard let probe = view as? Probe else { return }
            probe.leading = leading
            probe.distance = distance
            probe.place()
        }
    }

    /// A view that does nothing but reposition its window's buttons.
    private final class Probe: NSView {
        /// Where the first button's frame belongs, from the window's left edge.
        var leading: CGFloat = 0

        /// Where the buttons' centre belongs, below the window's top edge.
        var distance: CGFloat = 0

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            observe()
            place()
        }

        /// The buttons a window puts in its top-left corner.
        private static let kinds: [NSWindow.ButtonType] = [
            .closeButton, .miniaturizeButton, .zoomButton,
        ]

        /// Put the buttons where ``distance`` says, if there are any to move.
        ///
        /// Each button's own height is what the centring is done against, rather
        /// than a number written down here: they are 16 points tall today and that
        /// is not this view's business.
        func place() {
            guard distance > 0, let window else { return }

            for (index, kind) in Self.kinds.enumerated() {
                guard
                    let button = window.standardWindowButton(kind),
                    let container = button.superview
                else { continue }

                // Absolute, not a shift: this runs again on every window layout,
                // and nudging each button from wherever it currently is would walk
                // them across the title bar.
                let x = leading + CGFloat(index) * WindowButtons.spacing

                // The container is not flipped, so a larger `y` is higher up.
                let y = container.bounds.height - distance - button.frame.height / 2
                guard x != button.frame.origin.x || y != button.frame.origin.y else { continue }

                button.setFrameOrigin(NSPoint(x: x, y: y))
            }
        }

        /// Re-place the buttons whenever the window's own layout could have put
        /// them back.
        ///
        /// A resize is the common one; entering full screen and leaving it again
        /// rebuilds the title bar entirely.
        private func observe() {
            guard let window else { return }

            for name in [
                NSWindow.didResizeNotification,
                NSWindow.didEnterFullScreenNotification,
                NSWindow.didExitFullScreenNotification,
            ] {
                NotificationCenter.default.addObserver(
                    self,
                    selector: #selector(windowDidLayOut),
                    name: name,
                    object: window
                )
            }
        }

        @objc private func windowDidLayOut() {
            place()
        }

        deinit {
            NotificationCenter.default.removeObserver(self)
        }
    }
}
