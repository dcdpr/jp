import Testing

@testable import DriveKit

@Suite("Tree")
struct TreeTests {
    /// Options with the bounds wide open, so a test names only what it is about.
    private func options(
        prefix: String? = nil,
        maxMatches: Int = 100,
        maxDepth: Int = 20,
        maxSiblings: Int = 0,
        frames: Bool = false
    ) -> TreeOptions {
        return TreeOptions(
            pid: 0,
            identifierPrefix: prefix,
            maxMatches: maxMatches,
            maxDepth: maxDepth,
            maxSiblings: maxSiblings,
            frames: frames
        )
    }

    @Test("an unfiltered walk keeps every element")
    func keepsEverythingUnfiltered() throws {
        let root = FakeElement.sidebar(rowCount: 2)

        let tree = try #require(Tree.walk(from: root, options: options()))

        #expect(tree.role == "AXApplication")
        let outline = try #require(tree.children.first)
        #expect(outline.identifier == "sidebar.list")
        #expect(outline.children.count == 2)
    }

    /// The bug this replaced: with a sibling cap in force, a search for a row past
    /// the cap found nothing, because the cap dropped it before the filter saw it.
    @Test("a filtered walk finds a match past the sibling cap")
    func filterOutrunsTheSiblingCap() throws {
        let root = FakeElement.sidebar(rowCount: 50)

        let tree = try #require(
            Tree.walk(from: root, options: options(prefix: "sidebar.row.42", maxSiblings: 5))
        )

        let leaf = tree.children.first?.children.first?.children.first?.children.first
        #expect(leaf?.identifier == "sidebar.row.42")
    }

    /// Ancestors are kept so a match can be located, but only the ones leading to it.
    @Test("a filtered walk drops branches holding no match")
    func prunesUnmatchedBranches() throws {
        let root = FakeElement(
            role: "AXApplication",
            children: [
                FakeElement(role: "AXWindow", identifier: "other.window"),
                FakeElement(
                    role: "AXOutline",
                    identifier: "sidebar.list",
                    children: [FakeElement(role: "AXRow", identifier: "sidebar.row.0")]
                ),
            ]
        )

        let tree = try #require(Tree.walk(from: root, options: options(prefix: "sidebar.")))

        #expect(tree.children.count == 1)
        #expect(tree.children.first?.identifier == "sidebar.list")

        // The dropped window is still counted. A filtered read that silently
        // showed one child of two would have the reader believe the application
        // has one.
        #expect(tree.elidedChildren == 1)
    }

    @Test("a filtered walk with no match returns nothing")
    func returnsNothingWhenNothingMatches() {
        let root = FakeElement.sidebar(rowCount: 3)

        #expect(Tree.walk(from: root, options: options(prefix: "nope.")) == nil)
    }

    /// The budget is the bound that keeps a prefix search off the whole tree, so it
    /// has to actually stop the walk rather than only trim the output.
    @Test("the match budget stops the walk")
    func budgetStopsTheWalk() throws {
        let root = FakeElement.sidebar(rowCount: 500)

        let tree = try #require(
            Tree.walk(from: root, options: options(prefix: "sidebar.", maxMatches: 3))
        )

        // One match is the outline itself, leaving two rows.
        let outline = try #require(tree.children.first)
        #expect(outline.children.count == 2)

        // Reads, not results: a budget that trimmed the output while still visiting
        // every element would pass an assertion on the tree alone.
        let rows = try #require(root.children.first?.children)
        #expect(rows.dropFirst(3).allSatisfy { $0.reads == 0 })
    }

    @Test("the sibling cap reports what it skipped")
    func capReportsElidedChildren() throws {
        let root = FakeElement.sidebar(rowCount: 10)

        let tree = try #require(Tree.walk(from: root, options: options(maxSiblings: 4)))

        let outline = try #require(tree.children.first)
        #expect(outline.children.count == 4)
        #expect(outline.elidedChildren == 6)
    }

    @Test("a complete level reports no elision")
    func noElisionWhenComplete() throws {
        let root = FakeElement.sidebar(rowCount: 3)

        let tree = try #require(Tree.walk(from: root, options: options(maxSiblings: 10)))

        #expect(tree.children.first?.elidedChildren == nil)
    }

    /// The count is what separates a node stopped at the depth limit from a leaf.
    /// Without it the two render identically and a reader concludes the element
    /// has no children.
    @Test("the depth limit stops the descent and reports what it did not reach")
    func depthLimitStopsDescent() throws {
        let root = FakeElement.sidebar(rowCount: 2)

        let tree = try #require(Tree.walk(from: root, options: options(maxDepth: 1)))

        #expect(tree.children.first?.children.isEmpty == true)
        #expect(tree.children.first?.elidedChildren == 2)
    }

    /// Frames move whenever a window moves or a list scrolls, so they stay out
    /// unless asked for.
    @Test("frames are omitted by default")
    func framesAreOptIn() throws {
        let root = FakeElement(role: "AXWindow")
        root.attributes["AXFrame"] = "0.0,0.0 100.0x100.0"

        let without = try #require(Tree.walk(from: root, options: options()))
        #expect(without.frame == nil)

        let with = try #require(Tree.walk(from: root, options: options(frames: true)))
        #expect(with.frame == "0.0,0.0 100.0x100.0")
    }

    /// An attribute the element does not report must arrive as absent, not as the
    /// text of whatever error the accessibility API answered with.
    @Test("absent attributes are absent, not error text")
    func absentAttributesAreNull() throws {
        let root = FakeElement(role: "AXCell")

        let tree = try #require(Tree.walk(from: root, options: options()))

        #expect(tree.identifier == nil)
        #expect(tree.label == nil)
        #expect(tree.value == nil)
        #expect(tree.enabled == nil)
    }
}
