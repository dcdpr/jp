import AppKit
import Foundation

/// The state a driven run borrows from whoever is at the keyboard.
///
/// Which application is in front, and where the pointer is. Neither belongs to
/// the app under test: a synthesized gesture has to take both — mouse events go
/// to whatever is on top at a coordinate, and the ordering between applications
/// follows activation — and a run that takes them owes them back.
///
/// Deliberately not window geometry. A step that resizes a window did the thing
/// it was asked to do, and putting the window back would undo the effect under
/// test. What a run borrows is restored; what it was told to change is not.
///
/// Read and written separately rather than as one capture-and-restore pair, so a
/// caller composes what it needs and decides for itself when a restore is owed.
enum Ambient {
    /// The bundle identifier of the frontmost application.
    ///
    /// `nil` when there is none, or when it has no identifier — a process
    /// launched without a bundle has neither.
    static func frontmost() -> FrontmostReport {
        FrontmostReport(bundleID: NSWorkspace.shared.frontmostApplication?.bundleIdentifier)
    }

    /// Bring the application with `bundleID` back to the front.
    ///
    /// Through `NSWorkspace`, which asks the application to activate itself, so
    /// this needs no permission beyond launching one. Answers whether an
    /// application with that identifier was found to ask.
    static func activate(bundleID: String) -> FrontmostReport {
        guard
            let app = NSRunningApplication.runningApplications(withBundleIdentifier: bundleID)
                .first
        else {
            return FrontmostReport(bundleID: nil)
        }

        app.activate()
        return FrontmostReport(bundleID: bundleID)
    }

    /// Where the pointer is, in the coordinates a synthesized event uses.
    ///
    /// `NSEvent.mouseLocation` is bottom-left origin and screen coordinates are
    /// top-left, so the y is flipped here rather than at each call site. The
    /// height flipped against is the *main* screen's, which is what the window
    /// server measures global coordinates from.
    static func pointer() -> PointerReport {
        let location = NSEvent.mouseLocation
        let height = NSScreen.screens.first?.frame.height ?? 0

        return PointerReport(x: location.x, y: height - location.y)
    }

    /// Put the pointer back at `point`.
    ///
    /// Warped rather than moved: `CGWarpMouseCursorPosition` relocates the cursor
    /// without synthesizing motion, so nothing under it takes a hover, and no
    /// application sees a gesture it has to interpret.
    static func movePointer(to point: CGPoint) -> PointerReport {
        CGWarpMouseCursorPosition(point)
        return PointerReport(x: point.x, y: point.y)
    }
}

/// Which application is in front.
struct FrontmostReport: Encodable, Equatable {
    let bundleID: String?

    private enum CodingKeys: String, CodingKey {
        case bundleID = "bundle_id"
    }
}

/// Where the pointer is, in top-left-origin screen coordinates.
struct PointerReport: Encodable, Equatable {
    let x: CGFloat
    let y: CGFloat
}
