import ApplicationServices
import Foundation

/// Reads an application's menu bar.
///
/// Reported as a tree, because a menu is one: bar, then bar items, then menus,
/// then items. What makes it worth its own subcommand is the root — reaching the
/// menu bar from the application element takes an attribute that holds it, not a
/// walk through the window hierarchy.
///
/// Menu items are pressed with `act press`; they advertise `AXPress` where a list
/// row does not.
enum Menu {
    /// Read the menu bar of the application owning `pid`.
    static func read(pid: pid_t, options: TreeOptions) throws(DriveError) -> TreeNode {
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

        let app = AXElement.application(pid: pid)

        guard let bar = app.elements(kAXMenuBarAttribute).first else {
            throw DriveError(
                kind: .notFound,
                message: "the application reports no menu bar",
                hint: "an agent or accessory application has none"
            )
        }

        guard let tree = Tree.walk(from: bar, options: options) else {
            throw DriveError(
                kind: .notFound,
                message: "the menu bar held nothing matching",
                hint: nil
            )
        }

        return tree
    }
}
