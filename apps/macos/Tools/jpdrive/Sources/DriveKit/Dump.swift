import ApplicationServices
import Foundation

/// One element of an application's accessibility tree, with everything it reports.
struct DumpNode: Encodable, Equatable {
    /// `AXRole`, lifted out of the attributes because it is what a reader scans
    /// for.
    let role: String

    /// Every attribute the element reports, minus the two that only lead back into
    /// the tree, sorted by name.
    let attributes: [Attribute]

    /// Actions the element accepts, such as `AXPress`.
    let actions: [String]

    let children: [DumpNode]

    /// How many children were dropped to keep the walk bounded.
    ///
    /// Absent when every child was walked. A sidebar of a thousand conversations
    /// repeats one row shape a thousand times, so the count is the useful part and
    /// the repetition is not.
    let elidedChildren: Int?

    /// Set when the accessibility API refused to read this element.
    ///
    /// Absent otherwise. Without it an element that could not be read reports no
    /// attributes and no children, which is exactly how an element that has
    /// neither reports — and a reader has no way to tell a gap in the walk from
    /// a leaf.
    let unreadable: Bool?
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

    static func node<E: Element>(_ element: E, depth: Int, options: DumpOptions) -> DumpNode {
        let reported = element.reportedAttributes(settable: options.settable)
        let attributes = (reported ?? [])
            .filter { !structuralAttributes.contains($0.name) }

        // Every child the element has, whether or not this walk descends into it.
        // Counted before the depth cap is applied, because a node stopped at the
        // cap is otherwise indistinguishable from a leaf.
        let reading = element.read([])
        let available = reading.children
        let descend = depth < options.maxDepth ? available : []
        let walked =
            options.maxSiblings > 0
            ? Array(descend.prefix(options.maxSiblings))
            : descend
        let elided = available.count - walked.count
        let unreadable = reported == nil || reading.failed

        return DumpNode(
            role: attributes.first { $0.name == kAXRoleAttribute }?.value ?? "<none>",
            attributes: attributes,
            actions: element.actions,
            children: walked.map { node($0, depth: depth + 1, options: options) },
            elidedChildren: elided > 0 ? elided : nil,
            unreadable: unreadable ? true : nil
        )
    }
}
