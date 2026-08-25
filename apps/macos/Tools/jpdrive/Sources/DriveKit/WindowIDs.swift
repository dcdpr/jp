import CoreGraphics
import Foundation

/// A window the window server can be told to capture.
struct CaptureWindow: Encodable, Equatable {
    /// The window server's identifier, in the form `screencapture -l` takes.
    let id: CGWindowID

    /// The window's title, or `nil` when this process holds no Screen Recording
    /// grant: the window server withholds other applications' titles until it
    /// does.
    let title: String?

    let width: Int
    let height: Int
}

/// What `jpdrive windowid` observed.
struct WindowIDReport: Encodable, Equatable {
    /// Whether this process may read other applications' screen content.
    ///
    /// Enumerating windows needs no grant, so a report can list windows that
    /// cannot be captured. A caller that acts on the list without reading this
    /// gets a picture of the desktop where it expected a window.
    let screenRecording: Bool

    /// The application's capturable windows, front to back.
    let windows: [CaptureWindow]

    /// Windows the application has that the screen is not currently showing.
    ///
    /// Reported separately because such a window is absent from every on-screen
    /// enumeration and from the accessibility tree, so an app that has one and
    /// nothing else is indistinguishable from an app with no window at all —
    /// except by asking for every window regardless, which is this list.
    ///
    /// Says nothing about *why* it is not showing. Minimized, hidden, and on
    /// another Space all land here, and the window server's list carries no flag
    /// separating them. `jpdrive windows` reads `AXMinimized` from the
    /// accessibility tree, which distinguishes the first of the three.
    let offScreen: [CaptureWindow]
}

/// Resolves an application's window-server identifiers.
///
/// Separate from `Windows`, which reads the accessibility tree: the two answer
/// different questions and neither identifier converts into the other. An
/// accessibility window has a title and a frame but no number the capture tools
/// accept, and a window-server window has that number but nothing structural.
enum WindowIDs {
    /// The layer ordinary application windows sit on.
    ///
    /// Everything else the window server reports for an application is chrome —
    /// tooltips, drag images, the shadow behind a menu — and capturing one of
    /// those instead of the window is a silent wrong answer rather than a
    /// failure.
    static let normalLayer = 0

    /// The capturable windows of the application owning `pid`.
    static func read(pid: pid_t) throws(DriveError) -> WindowIDReport {
        guard ProcessTable.record(for: pid) != nil else {
            throw DriveError(
                kind: .appNotRunning,
                message: "no process is running under pid \(pid)",
                hint: "start the app, then pass its pid: --pid $(pgrep -f JP.app)"
            )
        }

        let onScreen =
            CGWindowListCopyWindowInfo(
                [.optionOnScreenOnly, .excludeDesktopElements],
                kCGNullWindowID
            ) as? [[String: Any]] ?? []

        // Every window, not just the ones on screen. The difference between the
        // two lists is what says a window exists somewhere the screen is not
        // showing it.
        let everywhere =
            CGWindowListCopyWindowInfo(
                [.excludeDesktopElements],
                kCGNullWindowID
            ) as? [[String: Any]] ?? []

        let here = capturable(from: onScreen, pid: pid)
        let all = capturable(from: everywhere, pid: pid)
        let shown = Set(here.map(\.id))

        // The preflight variant, never the requesting one: raising the system's
        // permission dialog from a background tool leaves a prompt nobody is
        // watching, in front of the app being measured.
        return WindowIDReport(
            screenRecording: CGPreflightScreenCaptureAccess(),
            windows: here,
            offScreen: all.filter { !shown.contains($0.id) }
        )
    }

    /// The windows in `listed` that belong to `pid` and can be captured,
    /// in the order the window server reported them, which is front to back.
    ///
    /// A window with no area is dropped: `AppKit` keeps zero-sized windows
    /// around for panels that have never been shown, and capturing one produces
    /// an empty file.
    static func capturable(from listed: [[String: Any]], pid: pid_t) -> [CaptureWindow] {
        return listed.compactMap { window -> CaptureWindow? in
            guard integer(window[kCGWindowOwnerPID as String]) == Int(pid),
                integer(window[kCGWindowLayer as String]) == normalLayer,
                let number = integer(window[kCGWindowNumber as String]),
                let id = CGWindowID(exactly: number),
                let bounds = window[kCGWindowBounds as String] as? [String: Any],
                let width = integer(bounds["Width"]),
                let height = integer(bounds["Height"]),
                width > 0, height > 0
            else {
                return nil
            }

            return CaptureWindow(
                id: id,
                title: window[kCGWindowName as String] as? String,
                width: width,
                height: height
            )
        }
    }

    /// One of the window server's numbers, whichever numeric type it arrives as.
    ///
    /// The list holds `CFNumber`s in untyped dictionaries. Bridged, those cast to
    /// `Int` while the value is whole and only to `Double` otherwise, which is a
    /// distinction window bounds can cross: a window on a scaled display sits at
    /// fractional points.
    private static func integer(_ value: Any?) -> Int? {
        if let int = value as? Int { return int }
        if let double = value as? Double { return Int(double) }
        return nil
    }
}
