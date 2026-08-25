import AppKit
import SwiftUI

/// Keeps a driven window reachable whichever Space is on screen.
///
/// macOS remembers which Space an application's windows belong to, keyed by
/// bundle identifier. Each debug slot runs its own copy of the app under its own
/// identifier — which is what isolates window state and the recents list — so a
/// slot's copy can acquire a Space assignment of its own and go on reopening
/// there. The assignment lives in the window server, not in the slot's state
/// directory, so nothing the harness controls can clear it.
///
/// A window on a Space that is not showing is not merely out of reach of a
/// synthesized click: it is absent from the accessibility tree entirely. Every
/// step a driver takes fails, and it fails as `identifier_not_found` — which
/// reads like a view that was never built rather than a window sitting one Space
/// away.
///
/// `canJoinAllSpaces` makes the window present wherever the person looking at it
/// happens to be, so the tree finds it and its frame means what the screen shows.
/// Activating the app would also work and would steal focus on every launch,
/// which a harness that deliberately launches in the background must not do.
///
/// Add it as a background of a window's content:
///
/// ```swift
/// content.background(DebugSpaces.joinEverySpace())
/// ```
enum DebugSpaces {
    /// A view that puts its window on every Space, for a driven build.
    ///
    /// Draws nothing, and does nothing at all unless the app was launched with a
    /// debug state directory. A window that followed the Space in an app somebody
    /// installed would be a window that will not stay where it was put.
    static func joinEverySpace() -> some View {
        Joiner()
    }

    private struct Joiner: NSViewRepresentable {
        func makeNSView(context: Context) -> NSView {
            Probe()
        }

        func updateNSView(_ view: NSView, context: Context) {}
    }

    /// A view that does nothing but widen its window's Space membership.
    private final class Probe: NSView {
        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()

            guard DebugState.directory != nil, let window else { return }

            window.collectionBehavior.insert(.canJoinAllSpaces)
        }
    }
}
