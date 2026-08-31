import ApplicationServices
import Foundation

/// One element, as the driver reports it.
///
/// An absent field means the element does not report that attribute. An
/// `AXUnknown` row carries a label and no value; a text field carries both.
struct TreeNode: Encodable, Equatable {
    let role: String
    let identifier: String?
    let label: String?
    let value: String?
    let enabled: Bool?
    let focused: Bool?

    /// The element's frame in screen coordinates, only when frames were asked for.
    ///
    /// Left out by default: coordinates change whenever a window moves or a list
    /// scrolls, so including them turns every diff between two snapshots into
    /// noise.
    let frame: String?

    let actions: [String]
    let children: [TreeNode]

    /// How many of this element's children are missing from ``children``.
    ///
    /// Every reason a child goes missing is counted the same, because they answer
    /// one question: is there more here than I am looking at? The depth limit, the
    /// per-level sibling cap, the match budget running out, and a filter discarding
    /// a branch that held no match all leave the reader in the same position, and
    /// the last two are the easiest to mistake for an element having no children at
    /// all.
    let elidedChildren: Int?

    /// Set when the accessibility API refused to read this element.
    ///
    /// Absent otherwise. An element that could not be read reports no identifier,
    /// no label and no children, which is how a bare container reports too — so
    /// without this a gap in the walk reads as a plain element.
    let unreadable: Bool?
}

/// What to walk, and what to keep.
struct TreeOptions {
    let pid: pid_t

    /// Keep only elements whose identifier begins with this, along with the
    /// ancestors that lead to them. `nil` keeps everything.
    ///
    /// A prefix rather than an exact match, because the useful question to ask of a
    /// tree is "what is under `sidebar.`". Acting on an element is the opposite
    /// case and matches exactly.
    let identifierPrefix: String?

    /// How many matches to find before stopping.
    ///
    /// This is the bound that matters. Every identifier in this app's sidebar sits
    /// on a leaf, so a prefix search cannot prune on the way down and an unbounded
    /// one visits every element in the application. Stopping at a handful of
    /// matches answers "what does the sidebar look like" for the cost of the first
    /// handful rather than of all thousand.
    ///
    /// Set this to `1` when looking up one identifier already known, or the walk
    /// continues past it looking for a second.
    let maxMatches: Int

    let maxDepth: Int
    let maxSiblings: Int
    let frames: Bool

    /// Whether to ask each kept element what it can be asked to do.
    ///
    /// Off by default because it is a round-trip per element and most callers
    /// discard the answer. A reading of a whole application is thousands of
    /// elements, so this is the difference between one accessibility call per
    /// node and two.
    let actions: Bool

    /// Whether to walk into the menu bar.
    ///
    /// Off by default because most of what hangs off it belongs to macOS rather
    /// than to the app — the Apple menu, Services, the window tiling submenus —
    /// and it runs to a couple of hundred elements around the handful
    /// describing the window.
    ///
    /// The bar itself is still reported, with its children counted as elided,
    /// so a reader can see it is there and ask for it.
    let menus: Bool
}

/// Reads an application's accessibility tree into something a person can scan.
///
/// Where [`Dump`](Dump) reports every attribute of every element for design work,
/// this reports the handful that identify and describe an element, and prunes
/// branches holding nothing that matched.
enum Tree {
    /// Attributes read for every node, in one batch. Order matters, since values
    /// come back positionally.
    static let batch = [
        kAXRoleAttribute,
        kAXIdentifierAttribute,
        AXElement.attributedDescription,
        kAXDescriptionAttribute,
        kAXTitleAttribute,
        kAXValueAttribute,
        kAXEnabledAttribute,
        kAXFocusedAttribute,
        "AXFrame",
    ]

    /// Walk the tree of the application owning `options.pid`.
    ///
    /// Returns `nil` when a prefix was given and nothing matched it.
    static func read(_ options: TreeOptions) throws(DriveError) -> TreeNode? {
        guard ProcessTable.record(for: options.pid) != nil else {
            throw DriveError(
                kind: .appNotRunning,
                message: "no process is running under pid \(options.pid)",
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

        return walk(from: AXElement.application(pid: options.pid), options: options)
    }

    /// Walk from `root`, spending a fresh match budget.
    static func walk<E: Element>(from root: E, options: TreeOptions) -> TreeNode? {
        // An unfiltered walk has no matches to count, so the budget only bounds a
        // filtered one.
        var remaining = options.identifierPrefix == nil ? Int.max : options.maxMatches
        return node(root, depth: 0, options: options, remaining: &remaining)
    }

    private static func node<E: Element>(
        _ element: E,
        depth: Int,
        options: TreeOptions,
        remaining: inout Int
    ) -> TreeNode? {
        let reading = element.read(batch)
        let text = reading.text

        let identifier = text[1]

        let matches =
            options.identifierPrefix.map { identifier?.hasPrefix($0) ?? false } ?? false
        if matches {
            remaining -= 1
        }

        // Every child the element has, whether or not this walk descends into it.
        // The count is what tells a reader there is more here; without it a node
        // stopped at the depth limit is indistinguishable from a leaf.
        let available = reading.children

        // The menu bar is skipped here rather than filtered out of the rendered
        // result, so the elements under it are never read at all. Reading and
        // discarding them is the same couple of hundred round-trips as reading
        // and showing them.
        let unwalkedMenus = !options.menus && text[0] == "AXMenuBar"
        let all = depth < options.maxDepth && !unwalkedMenus ? available : []

        // The sibling cap is for reading an unfiltered tree, where every level is
        // worth seeing but a thousand copies of one row are not. Under a filter the
        // match budget does the bounding instead: a cap here would hide the eight
        // hundredth row from a search that named it.
        let capped = options.identifierPrefix == nil && options.maxSiblings > 0

        var children: [TreeNode] = []
        var visited = 0

        for child in all {
            guard remaining > 0 else { break }
            guard !capped || visited < options.maxSiblings else { break }
            visited += 1

            guard
                let node = node(
                    child, depth: depth + 1, options: options, remaining: &remaining)
            else { continue }
            children.append(node)
        }

        // A branch is kept when it matches, or when something under it does. The
        // ancestors are what make a match locatable rather than a bare hit.
        guard matches || !children.isEmpty || options.identifierPrefix == nil else {
            return nil
        }

        return TreeNode(
            role: text[0] ?? "<none>",
            identifier: identifier,
            label: text[2] ?? text[3] ?? text[4],
            value: text[5],
            enabled: text[6].axFlag,
            focused: text[7].axFlag,
            frame: options.frames ? text[8] : nil,
            // Read only for a node being kept, and only when asked for: this is
            // a round-trip of its own, and a caller that is not going to show
            // them should not pay for them.
            actions: options.actions ? element.actions : [],
            children: children,
            elidedChildren: available.count > children.count
                ? available.count - children.count
                : nil,
            unreadable: reading.failed ? true : nil
        )
    }
}
