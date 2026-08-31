import ApplicationServices

@testable import DriveKit

/// An accessibility element that is not one.
///
/// Reference semantics on purpose: a step writes to an element and then reads it
/// back, and a test asserts on what was written. With a value type the write would
/// land on a copy and every such assertion would pass vacuously.
final class FakeElement: Element {
    /// Attribute values by name. A name that is absent here reads as `nil`, which
    /// is what the real implementation answers for an attribute the element does
    /// not report.
    var attributes: [String: String]

    /// Attribute names this element accepts writes for.
    let settable: Set<String>

    /// Counts every read of ``actions``, so a test can pin that a walk which was
    /// not asked for them never asked the element.
    ///
    /// Each read is an accessibility round-trip of its own against a real
    /// element, so "the answer came back empty" and "nothing was asked" are
    /// different outcomes with the same output.
    private(set) var actionReads = 0

    private var storedActions: [String]

    var actions: [String] {
        actionReads += 1
        return storedActions
    }

    var children: [FakeElement]

    /// Counts every call to ``read(_:)``, so a test can pin how much of a tree a
    /// walk touched rather than only what it returned.
    private(set) var reads = 0

    /// What `setFlag` should answer, for exercising a refused write.
    var writeStatus: AXError = .success

    /// Make every read of this element fail as a whole.
    ///
    /// What the accessibility API answers for an element whose application is
    /// busy or exiting. Distinct from an element that simply reports nothing:
    /// that is the confusion the `failed` flag exists to prevent, so a fake has
    /// to be able to produce both.
    var readFails = false

    /// Accept writes and discard them.
    ///
    /// The accessibility API lets a target answer `success` and then do nothing,
    /// which is why a step reads back rather than trusting the status. Without this
    /// there is no way to tell a driver that reads back from one that pretends to.
    var ignoresWrites = false

    /// Called during every ``read(_:)``, after ``reads`` is incremented and before
    /// children are handed back.
    ///
    /// This is how a test makes an element appear partway through a wait. A fixture
    /// that has the element from the start cannot tell polling from a single lucky
    /// look.
    var onRead: ((FakeElement) -> Void)?

    /// Element-valued attributes, such as `AXWindows` and `AXMenuBar`.
    var related: [String: [FakeElement]] = [:]

    /// Point-valued attributes, such as `AXActivationPoint`.
    var points: [String: CGPoint] = [:]

    /// Size-valued attributes, such as `AXSize`.
    var sizes: [String: CGSize] = [:]

    /// Actions performed on this element, in order.
    private(set) var performed: [String] = []

    /// What `perform` should answer, for exercising a refused action.
    var performStatus: AXError = .success

    init(
        role: String,
        identifier: String? = nil,
        label: String? = nil,
        settable: Set<String> = [],
        actions: [String] = [],
        children: [FakeElement] = []
    ) {
        self.attributes = [kAXRoleAttribute: role]
        self.attributes[kAXIdentifierAttribute] = identifier
        self.attributes[AXElement.attributedDescription] = label
        self.settable = settable
        self.storedActions = actions
        self.children = children
    }

    func read(_ names: [String]) -> Reading<FakeElement> {
        reads += 1
        onRead?(self)

        guard !readFails else {
            return Reading(
                text: Array(repeating: nil, count: names.count),
                children: [],
                failed: true
            )
        }

        return Reading(text: names.map { attributes[$0] }, children: children)
    }

    func reportedAttributes(settable: Bool) -> [Attribute]? {
        guard !readFails else { return nil }

        return attributes.keys.sorted().map { name in
            Attribute(
                name: name,
                value: attributes[name] ?? "<null>",
                settable: settable ? isSettable(name) : nil
            )
        }
    }

    func isSettable(_ name: String) -> Bool {
        return settable.contains(name)
    }

    func flag(_ name: String) -> Bool? {
        switch attributes[name] {
        case "1": return true
        case "0": return false
        default: return nil
        }
    }

    func setFlag(_ name: String, _ value: Bool) -> AXError {
        guard writeStatus == .success else { return writeStatus }
        guard !ignoresWrites else { return .success }
        attributes[name] = value ? "1" : "0"
        return .success
    }

    func setText(_ name: String, _ value: String) -> AXError {
        guard writeStatus == .success else { return writeStatus }
        guard !ignoresWrites else { return .success }
        attributes[name] = value
        return .success
    }

    func perform(_ action: String) -> AXError {
        guard performStatus == .success else { return performStatus }
        performed.append(action)
        return .success
    }

    func point(_ name: String) -> CGPoint? {
        return points[name]
    }

    func size(_ name: String) -> CGSize? {
        return sizes[name]
    }

    func setSize(_ name: String, _ value: CGSize) -> AXError {
        guard writeStatus == .success else { return writeStatus }
        guard !ignoresWrites else { return .success }
        sizes[name] = value
        return .success
    }

    func elements(_ name: String) -> [FakeElement] {
        return related[name] ?? []
    }
}

extension FakeElement {
    /// The sidebar shape this app actually produces, at whatever size a test needs.
    ///
    /// Three elements per conversation, with the identifier on the leaf and
    /// `AXSelected` writable only on the row. Reproducing that here is the point:
    /// the driver has to address one element and act on another.
    static func sidebar(rowCount: Int) -> FakeElement {
        let rows = (0..<rowCount).map { index in
            FakeElement(
                role: "AXRow",
                settable: [kAXSelectedAttribute],
                actions: ["AXShowDefaultUI"],
                children: [
                    FakeElement(
                        role: "AXCell",
                        children: [
                            FakeElement(
                                role: "AXUnknown",
                                identifier: "sidebar.row.\(index)",
                                label: "Conversation \(index), 4 events"
                            )
                        ]
                    )
                ]
            )
        }

        return FakeElement(
            role: "AXApplication",
            children: [
                FakeElement(
                    role: "AXOutline",
                    identifier: "sidebar.list",
                    label: "Conversations",
                    actions: ["AXShowMenu"],
                    children: rows
                )
            ]
        )
    }
}
