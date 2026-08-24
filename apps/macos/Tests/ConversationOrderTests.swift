import Testing

@testable import JP

@Suite("ConversationOrder")
struct ConversationOrderTests {
    /// A conversation with a fixed ID, pinned or not.
    ///
    /// Nothing here reads the timestamps or the event count, so they are the same
    /// for every one: what the ordering depends on is the pin and the position.
    private func conversation(_ id: String, pinned: Bool = false) -> ConversationSummary {
        ConversationSummary(
            id: id,
            title: "Conversation \(id)",
            lastActivatedAt: "2026-08-01T10:00:00Z",
            pinnedAt: pinned ? "2026-08-02T09:00:00Z" : nil,
            eventsCount: 3
        )
    }

    private func ids(_ conversations: [ConversationSummary]) -> [String] {
        ConversationOrder.pinnedFirst(conversations).map(\.id)
    }

    @Test("lifts a pinned conversation above the unpinned ones")
    func pinnedGoesFirst() {
        let listing = [
            conversation("1"),
            conversation("2"),
            conversation("3", pinned: true),
        ]

        #expect(ids(listing) == ["3", "1", "2"])
    }

    /// The library reports conversations most recently active first, and that
    /// order has to survive inside each group: pinning is meant to lift one
    /// conversation, not to reshuffle the rest.
    @Test("keeps the given order inside each group")
    func orderWithinGroupsIsKept() {
        let listing = [
            conversation("1"),
            conversation("2", pinned: true),
            conversation("3"),
            conversation("4", pinned: true),
        ]

        #expect(ids(listing) == ["2", "4", "1", "3"])
    }

    @Test("leaves a list with no pins exactly as it was")
    func noPinsChangesNothing() {
        let listing = [conversation("1"), conversation("2"), conversation("3")]

        #expect(ids(listing) == ["1", "2", "3"])
    }

    @Test("leaves a list of nothing but pins exactly as it was")
    func allPinnedChangesNothing() {
        let listing = [
            conversation("1", pinned: true),
            conversation("2", pinned: true),
        ]

        #expect(ids(listing) == ["1", "2"])
    }

    @Test("orders an empty list")
    func emptyList() {
        #expect(ConversationOrder.pinnedFirst([]).isEmpty)
    }

    private func bareRows(_ listing: [ConversationSummary], selecting: String?) -> Set<String> {
        ConversationOrder.rowsWithoutSeparator(in: listing, selecting: selecting)
    }

    /// Both lines touching the selection go, not just the one under it: a
    /// separator drawn above the selected row cuts across the top of its rounded
    /// fill just as visibly as one below cuts the bottom.
    @Test("drops the separator on the selected row and the one above it")
    func dropsBothSeparatorsTouchingTheSelection() {
        let listing = [conversation("1"), conversation("2"), conversation("3")]

        #expect(bareRows(listing, selecting: "2") == ["1", "2"])
    }

    /// There is no row above the first, so only its own line goes.
    @Test("drops one separator when the first row is selected")
    func firstRowHasNothingAboveIt() {
        let listing = [conversation("1"), conversation("2")]

        #expect(bareRows(listing, selecting: "1") == ["1"])
    }

    @Test("draws every separator when nothing is selected")
    func noSelectionDropsNothing() {
        let listing = [conversation("1"), conversation("2")]

        #expect(bareRows(listing, selecting: nil).isEmpty)
    }

    /// A filter can hide the selected conversation while it stays selected, and
    /// the rows still on screen all keep their lines.
    @Test("draws every separator when the selection is not in the list")
    func selectionOutsideTheListDropsNothing() {
        let listing = [conversation("1"), conversation("2")]

        #expect(bareRows(listing, selecting: "3").isEmpty)
    }
}
