/// The workspace the conversation-list tests run against.
///
/// A type of its own rather than statics on the suite, because a suite's own
/// `.sharedApp(...)` attribute cannot name the suite it is attached to: the
/// macro would have to resolve the type it is in the middle of expanding.
enum ConversationFixtures {
    /// Oldest activity, so it sorts last.
    static let readingList = FixtureConversation(
        id: "17251488000",
        title: "Reading list",
        lastActivatedAt: "2024-09-01 09:00:00.0",
        events: [
            FixtureConversation.userMessage(
                at: "2024-09-01 09:00:00.0", from: "Jean", "What is on the reading list?"),
            FixtureConversation.assistantMessage(
                at: "2024-09-01 09:00:01.0", "Three books and a paper."),
        ]
    )

    static let configPipeline = FixtureConversation(
        id: "17251488010",
        title: "Config pipeline",
        lastActivatedAt: "2024-09-02 09:00:00.0",
        events: [
            FixtureConversation.userMessage(
                at: "2024-09-02 09:00:00.0", from: "Jean", "How does the config pipeline layer?"
            ),
            FixtureConversation.assistantMessage(
                at: "2024-09-02 09:00:01.0", "Later layers win, field by field."),
        ]
    )

    /// Newest activity, so it sorts first.
    static let releaseNotes = FixtureConversation(
        id: "17251488020",
        title: "Release notes",
        lastActivatedAt: "2024-09-03 09:00:00.0",
        events: [
            FixtureConversation.userMessage(
                at: "2024-09-03 09:00:00.0", from: "Jean", "Draft the release notes."),
            FixtureConversation.assistantMessage(
                at: "2024-09-03 09:00:01.0", "Drafted, with one open question."),
            FixtureConversation.userMessage(
                at: "2024-09-03 09:00:02.0", from: "Jean", "Answer it yourself."),
            FixtureConversation.assistantMessage(
                at: "2024-09-03 09:00:03.0", "Answered."),
        ]
    )

    /// ``readingList``, pinned.
    ///
    /// The oldest of the three, so a list showing it first can only be showing it
    /// there because it is pinned.
    static let pinnedReadingList = FixtureConversation(
        id: readingList.id,
        title: readingList.title,
        lastActivatedAt: readingList.lastActivatedAt,
        pinnedAt: "2024-09-04 09:00:00.0",
        events: readingList.events
    )

    /// One conversation tall enough to scroll, for the tests about re-wrapping.
    ///
    /// Prose rather than a repeated line, because the thing under test is text
    /// finding new line breaks at a new width: a paragraph of one word repeated
    /// wraps at the same places whatever the width, and would reflow invisibly.
    static let longRead = FixtureConversation(
        id: "17251488030",
        title: "Long read",
        lastActivatedAt: "2024-09-05 09:00:00.0",
        events: [
            FixtureConversation.userMessage(
                at: "2024-09-05 09:00:00.0", from: "Jean", "Explain the layout pipeline."),
            FixtureConversation.assistantMessage(
                at: "2024-09-05 09:00:01.0", paragraphs(40)),
        ]
    )

    /// `count` paragraphs of varied prose, as one markdown message.
    ///
    /// Numbered so a reader of a failure can tell where in the document they are,
    /// and of uneven length so the line breaks are not all in the same column.
    private static func paragraphs(_ count: Int) -> String {
        (1...count)
            .map { index in
                """
                ## Section \(index)

                The layout pipeline measures what it is given and wraps it to the \
                width it is offered, which is why a window resize is a text \
                problem rather than a drawing one. Paragraph \(index) exists to \
                take up enough room that the document is taller than any window \
                showing it.
                """
            }
            .joined(separator: "\n\n")
    }

    /// A workspace holding all three.
    static func make() throws -> WorkspaceFixture {
        try WorkspaceFixture.make(conversations: [readingList, configPipeline, releaseNotes])
    }

    /// A workspace holding only ``longRead``.
    ///
    /// One conversation, so the sidebar publishes almost nothing and every
    /// synthesized event in the test is cheap.
    static func makeLongRead() throws -> WorkspaceFixture {
        try WorkspaceFixture.make(conversations: [longRead])
    }

    /// The same three, with the oldest one pinned.
    static func makeWithPinnedOldest() throws -> WorkspaceFixture {
        try WorkspaceFixture.make(
            conversations: [pinnedReadingList, configPipeline, releaseNotes])
    }
}
