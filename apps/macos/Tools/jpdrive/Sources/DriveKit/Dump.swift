import ApplicationServices
import Foundation

/// One accessibility attribute, as reported name and rendered value.
///
/// A list of pairs rather than a dictionary, so attribute names reach the JSON
/// exactly as the accessibility API spells them. `JSONEncoder`'s snake-case key
/// strategy rewrites dictionary keys, which would turn `AXIdentifier` into
/// `ax_identifier` and make the dump a poor record of what the app reports.
struct DumpAttribute: Encodable {
    let name: String
    let value: String

    /// Whether the accessibility API reports this attribute as writable, when
    /// settability was asked for.
    ///
    /// This decides how the driver changes state. Writing `AXSelected` on a row is
    /// deterministic; synthesizing a click at a screen coordinate depends on the
    /// window being frontmost and unobscured.
    ///
    /// Absent unless requested: answering it costs one round-trip per attribute,
    /// which doubles the cost of a walk.
    let settable: Bool?
}

/// One element of an application's accessibility tree, with everything it reports.
struct DumpNode: Encodable {
    /// `AXRole`, lifted out of the attributes because it is what a reader scans
    /// for.
    let role: String

    /// Every attribute the element reports, minus the two that only lead back into
    /// the tree, sorted by name.
    let attributes: [DumpAttribute]

    /// Actions the element accepts, such as `AXPress`.
    let actions: [String]

    let children: [DumpNode]

    /// How many children were dropped to keep the walk bounded.
    ///
    /// Absent when every child was walked. A sidebar of a thousand conversations
    /// repeats one row shape a thousand times, so the count is the useful part and
    /// the repetition is not.
    let elidedChildren: Int?
}

/// Walks an application's accessibility tree and reports everything it finds.
///
/// This is a design instrument. SwiftUI's mapping onto accessibility elements is
/// undocumented and not one-to-one, so decisions about how to address and act on
/// an element are made by reading a real dump rather than by predicting where a
/// `.accessibilityIdentifier` lands.
///
/// Unfiltered by intent: every attribute of every element it visits, so nothing
/// that turns out to matter has been quietly dropped. [`Tree`](Tree) is the
/// filtered counterpart for everyday use.
enum Dump {
    /// Walk the tree rooted at the application owning `options.pid`.
    static func walk(_ options: DumpOptions) throws(DriveError) -> DumpNode {
        guard ProcessTable.record(for: options.pid) != nil else {
            throw DriveError(
                kind: .appNotRunning,
                message: "no process is running under pid \(options.pid)",
                hint: "start the app, then pass its pid: --pid $(pgrep -f JP.app)"
            )
        }

        // Checked up front rather than reported per element: without the grant
        // every read fails, and a tree of identical refusals says less than one
        // error naming the pane that fixes it.
        guard AXIsProcessTrusted() else {
            throw DriveError(
                kind: .notPermitted,
                message: "not trusted to read another application's accessibility tree",
                hint: DriveError.accessibilityHint
            )
        }

        return node(AXElement.application(pid: options.pid), depth: 0, options: options)
    }

    /// Attributes that only lead back into the tree, and so are not recorded.
    ///
    /// `AXChildren` is what the walk recurses into, and `AXParent` points at the
    /// element that just reported this one.
    private static let structuralAttributes: Set<String> = [
        kAXChildrenAttribute,
        kAXParentAttribute,
    ]

    private static func node(_ element: AXElement, depth: Int, options: DumpOptions) -> DumpNode
    {
        let names =
            element.names()
            .filter { !structuralAttributes.contains($0) }
            .sorted()

        let attributes = zip(names, element.values(names)).map { name, value in
            DumpAttribute(
                name: name,
                value: value.map(AXElement.text) ?? "<null>",
                settable: options.settable ? element.isSettable(name) : nil
            )
        }

        let all = depth < options.maxDepth ? element.children : []
        let walked = options.maxSiblings > 0 ? Array(all.prefix(options.maxSiblings)) : all

        return DumpNode(
            role: attributes.first { $0.name == kAXRoleAttribute }?.value ?? "<none>",
            attributes: attributes,
            actions: element.actions,
            children: walked.map { node($0, depth: depth + 1, options: options) },
            elidedChildren: all.count > walked.count ? all.count - walked.count : nil
        )
    }
}
