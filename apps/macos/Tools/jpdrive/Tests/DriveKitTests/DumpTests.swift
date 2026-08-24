import ApplicationServices
import Testing

@testable import DriveKit

@Suite("Dump")
struct DumpTests {
    /// Options with the bounds wide open, so a test names only what it is about.
    private func options(
        maxDepth: Int = 20,
        maxSiblings: Int = 0,
        settable: Bool = false
    ) -> DumpOptions {
        return DumpOptions(
            pid: 0,
            maxDepth: maxDepth,
            maxSiblings: maxSiblings,
            settable: settable
        )
    }

    @Test("an unbounded walk reports every element and elides nothing")
    func walksEverything() throws {
        let root = FakeElement.sidebar(rowCount: 2)

        let node = Dump.node(root, depth: 0, options: options())

        #expect(node.role == "AXApplication")
        #expect(node.elidedChildren == nil)

        let outline = try #require(node.children.first)
        #expect(outline.role == "AXOutline")
        #expect(outline.children.count == 2)
        #expect(outline.elidedChildren == nil)
    }

    /// The bug this file was written for: the depth cap emptied the child list
    /// before the count was taken, so a node stopped at the cap reported no
    /// children and no elision — identical to a leaf, which is the reading
    /// `elidedChildren` exists to prevent.
    @Test("a node stopped at the depth limit reports what it did not walk")
    func depthLimitReportsElision() throws {
        let root = FakeElement.sidebar(rowCount: 3)

        let node = Dump.node(root, depth: 0, options: options(maxDepth: 1))

        let outline = try #require(node.children.first)
        #expect(outline.children.isEmpty)
        #expect(outline.elidedChildren == 3)
    }

    @Test("a node under the sibling cap reports what it skipped")
    func siblingCapReportsElision() throws {
        let root = FakeElement.sidebar(rowCount: 10)

        let node = Dump.node(root, depth: 0, options: options(maxSiblings: 4))

        let outline = try #require(node.children.first)
        #expect(outline.children.count == 4)
        #expect(outline.elidedChildren == 6)
    }

    /// A genuine leaf and a truncated node must not render the same way, which is
    /// the whole point of the field.
    @Test("a leaf reports no elision")
    func leafReportsNothing() {
        let node = Dump.node(FakeElement(role: "AXButton"), depth: 0, options: options())

        #expect(node.children.isEmpty)
        #expect(node.elidedChildren == nil)
    }

    /// `AXChildren` is what the walk recurses into and `AXParent` points back at
    /// the element that reported this one, so neither is worth recording.
    @Test("the attributes that only lead back into the tree are dropped")
    func dropsStructuralAttributes() {
        let element = FakeElement(role: "AXRow", identifier: "row")
        element.attributes[kAXChildrenAttribute] = "<2 AXUIElement>"
        element.attributes[kAXParentAttribute] = "<AXUIElement>"

        let node = Dump.node(element, depth: 0, options: options())
        let names = node.attributes.map(\.name)

        #expect(!names.contains(kAXChildrenAttribute))
        #expect(!names.contains(kAXParentAttribute))
        #expect(names.contains(kAXIdentifierAttribute))
    }

    @Test("settability is answered only when it was asked for")
    func settabilityIsOptIn() throws {
        let element = FakeElement(
            role: "AXRow", settable: [kAXSelectedAttribute])
        element.attributes[kAXSelectedAttribute] = "0"

        let without = Dump.node(element, depth: 0, options: options())
        #expect(without.attributes.allSatisfy { $0.settable == nil })

        let with = Dump.node(element, depth: 0, options: options(settable: true))
        let selected = try #require(with.attributes.first { $0.name == kAXSelectedAttribute })
        #expect(selected.settable == true)
    }

    /// An element the accessibility API refused reports no attributes and no
    /// children, which is exactly how an empty element reports. Without the flag
    /// a gap in the walk is indistinguishable from a leaf that is really there.
    @Test("an element that could not be read is marked rather than shown empty")
    func marksAnUnreadableElement() throws {
        let broken = FakeElement(role: "AXGroup", identifier: "gone")
        broken.readFails = true
        let root = FakeElement(role: "AXApplication", children: [broken])

        let node = Dump.node(root, depth: 0, options: options())

        #expect(node.unreadable == nil)

        let child = try #require(node.children.first)
        #expect(child.unreadable == true)
        #expect(child.attributes.isEmpty)
        #expect(child.role == "<none>")
    }
}
