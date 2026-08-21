import Testing

@testable import JP

@Suite("ConversationFilter")
struct ConversationFilterTests {
    /// Fixed IDs and titles, so an assertion names exactly what it expects.
    private let conversations = [
        ConversationSummary(
            id: "17855681129",
            title: "Accessibility identifiers for driving",
            lastActivatedAt: "2026-08-01T10:00:00Z",
            pinnedAt: nil,
            eventsCount: 116
        ),
        ConversationSummary(
            id: "17855681250",
            title: "jpdrive: the accessibility driver",
            lastActivatedAt: "2026-08-01T09:00:00Z",
            pinnedAt: nil,
            eventsCount: 124
        ),
        ConversationSummary(
            id: "17855299562",
            title: "Café hours",
            lastActivatedAt: "2026-08-01T08:00:00Z",
            pinnedAt: nil,
            eventsCount: 3
        ),
        ConversationSummary(
            id: "17801582617",
            title: nil,
            lastActivatedAt: "2026-07-01T08:00:00Z",
            pinnedAt: nil,
            eventsCount: 4
        ),
    ]

    private func ids(_ query: String) -> [String] {
        return ConversationFilter.matches(conversations, query: query).map(\.id)
    }

    /// Clearing the box restores the list. An empty query meaning "match nothing"
    /// would empty the sidebar the moment somebody deleted what they typed.
    @Test("a blank query matches everything", arguments: ["", "   ", "\n"])
    func blankMatchesEverything(query: String) {
        #expect(ConversationFilter.matches(conversations, query: query).count == 4)
    }

    @Test("matches anywhere in the title, not only at the start")
    func matchesASubstring() {
        #expect(ids("driver") == ["17855681250"])
    }

    @Test("ignores case")
    func ignoresCase() {
        #expect(ids("ACCESSIBILITY") == ["17855681129", "17855681250"])
    }

    /// `localizedStandardContains` folds diacritics, which is what a person typing
    /// on a keyboard without the accent expects.
    @Test("ignores diacritics")
    func ignoresDiacritics() {
        #expect(ids("cafe") == ["17855299562"])
    }

    /// An untitled conversation shows a placeholder, and a row a person can read
    /// but not search for is a surprise.
    @Test("finds untitled conversations by their placeholder")
    func findsUntitled() {
        #expect(ids("untitled") == ["17801582617"])
    }

    /// Surrounding whitespace comes free with pasting and typing, and no title has
    /// a leading space to match anyway.
    @Test("trims the query")
    func trimsTheQuery() {
        #expect(ids("  driver  ") == ["17855681250"])
    }

    @Test("keeps the list in order")
    func preservesOrder() {
        #expect(ids("accessibility") == ["17855681129", "17855681250"])
    }

    @Test("matches nothing when nothing matches")
    func matchesNothing() {
        #expect(ids("zzz").isEmpty)
    }

    /// IDs are timestamps. Searching them would let a query hit rows with no
    /// visible reason, which reads as a bug rather than a feature.
    @Test("does not match on the conversation ID")
    func doesNotMatchIDs() {
        #expect(ids("17855681129").isEmpty)
    }

    /// The row and the filter have to agree about what an untitled conversation is
    /// called, or one of them is lying.
    @Test("the displayed title is the one searched")
    func displayTitleIsShared() {
        #expect(ConversationFilter.displayTitle(of: conversations[3]) == "Untitled")
        #expect(ConversationFilter.displayTitle(of: conversations[2]) == "Café hours")
    }
}
