import AppKit
import SwiftUI

/// Pins the window to a known frame, when the environment asks for one.
///
/// For tests, and inert otherwise: without `JP_WINDOW_FRAME` set, nothing here
/// runs and the window behaves as any other, remembering where it was left.
///
/// That memory is the problem it exists for. A window's frame is autosaved into
/// user defaults, which is a different mechanism from the saved application
/// state `-ApplePersistenceIgnoreState` disables — so a UI test suite can turn
/// off state restoration, as this one does, and still inherit the size the last
/// run left behind. A test that resizes the window then hands the next run a
/// different starting point, and one that grows the window eventually hands it
/// a window with nowhere left to grow.
struct FixedWindowFrame: ViewModifier {
    /// The frame to pin to, as `<width>x<height>`.
    ///
    /// `nonisolated` because a name is not UI state: a `ViewModifier` is
    /// main-actor isolated, which would otherwise follow this string everywhere
    /// it is read.
    nonisolated static let environmentKey = "JP_WINDOW_FRAME"

    /// The requested frame, if the environment names a usable one.
    private static var requested: CGSize? {
        guard let value = ProcessInfo.processInfo.environment[environmentKey] else {
            return nil
        }

        let parts = value.split(separator: "x")
        guard parts.count == 2,
            let width = Double(parts[0]),
            let height = Double(parts[1]),
            width > 0,
            height > 0
        else {
            return nil
        }

        return CGSize(width: width, height: height)
    }

    /// Gap left between the window and the edges of the screen it is placed
    /// against.
    ///
    /// Also the slack ``contentSize(pinning:within:)`` leaves, which is why it is
    /// larger than a margin needs to be: a content size that exactly filled the
    /// screen would still produce a frame wider than the screen once window
    /// chrome is added.
    nonisolated static let inset: CGFloat = 40

    /// `pin`, reduced to what a screen showing `visible` can display.
    ///
    /// A window pinned wider than the display keeps the width it was asked for
    /// and puts its right edge past the screen. Nothing can press an edge out
    /// there: a pointer stops at the display bounds, so a gesture aimed at it
    /// lands on whatever is at the edge instead.
    nonisolated static func contentSize(pinning pin: CGSize, within visible: CGRect) -> CGSize {
        CGSize(
            width: min(pin.width, visible.width - inset * 2),
            height: min(pin.height, visible.height - inset * 2)
        )
    }

    func body(content: Content) -> some View {
        guard let size = Self.requested else {
            return AnyView(content)
        }

        return AnyView(content.background(WindowPinner(size: size)))
    }
}

extension View {
    /// Pin the window to the frame `JP_WINDOW_FRAME` names, if it names one.
    func fixedWindowFrame() -> some View {
        modifier(FixedWindowFrame())
    }
}

/// Reaches the `NSWindow` behind a SwiftUI scene, to set its frame once.
private struct WindowPinner: NSViewRepresentable {
    let size: CGSize

    func makeNSView(context: Context) -> NSView {
        let view = WindowPinningView()
        view.pin = size
        return view
    }

    func updateNSView(_ view: NSView, context: Context) {}
}

/// A view that acts the moment it is put in a window.
///
/// `viewDidMoveToWindow` rather than `updateNSView`, which is called when SwiftUI
/// decides to and can run before the view has a window at all — and then not
/// again, if nothing else changes.
private final class WindowPinningView: NSView {
    var pin: CGSize?

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()

        guard let window, let pin else { return }

        // Emptying the autosave name is what stops this run from writing its
        // size back over the default the next one reads.
        _ = window.setFrameAutosaveName("")

        guard let visible = window.screen?.visibleFrame else {
            window.setContentSize(pin)
            return
        }

        window.setContentSize(FixedWindowFrame.contentSize(pinning: pin, within: visible))

        // Placed toward the left of the screen rather than centred, so the whole
        // window — both vertical edges included — is on screen. Read from the
        // frame rather than the content size, so window chrome is counted.
        window.setFrameOrigin(
            CGPoint(
                x: visible.minX + FixedWindowFrame.inset,
                y: visible.maxY - window.frame.height - FixedWindowFrame.inset
            )
        )
    }
}
