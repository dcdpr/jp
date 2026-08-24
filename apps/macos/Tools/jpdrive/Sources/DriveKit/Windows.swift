import ApplicationServices
import Foundation

/// One of an application's windows.
struct WindowSummary: Encodable, Equatable {
    let identifier: String?
    let title: String?

    /// Whether this is the application's main window.
    let main: Bool?

    let minimized: Bool?

    /// Position and size in screen coordinates.
    ///
    /// Included here, unlike in a tree, because a window's frame is what the
    /// listing is for: which window is where, and how big.
    let frame: String?
}

/// Lists an application's windows.
///
/// Separate from a tree walk because the useful facts about a window are its own —
/// which one is main, which is minimized, where it sits — rather than what it
/// contains.
enum Windows {
    /// Attributes read for every window, in one batch.
    static let batch = [
        kAXIdentifierAttribute,
        kAXTitleAttribute,
        kAXMainAttribute,
        kAXMinimizedAttribute,
        "AXFrame",
    ]

    /// List the windows of the application owning `pid`.
    static func read(pid: pid_t) throws(DriveError) -> [WindowSummary] {
        guard ProcessTable.record(for: pid) != nil else {
            throw DriveError(
                kind: .appNotRunning,
                message: "no process is running under pid \(pid)",
                hint: "start the app, then pass its pid: --pid $(pgrep -f JP.app)"
            )
        }

        guard AXIsProcessTrusted() else {
            throw DriveError(
                kind: .notPermitted,
                message: "not trusted to read another application's accessibility tree",
                hint: DriveError.accessibilityHint
            )
        }

        return list(of: AXElement.application(pid: pid))
    }

    /// The windows an application element reports.
    ///
    /// An application with no windows answers an empty list, which is a state a
    /// running app can legitimately be in.
    static func list<E: Element>(of app: E) -> [WindowSummary] {
        return app.elements(kAXWindowsAttribute).map { window in
            let text = window.read(batch).text

            return WindowSummary(
                identifier: text[0],
                title: text[1],
                main: text[2].axFlag,
                minimized: text[3].axFlag,
                frame: text[4]
            )
        }
    }
}
