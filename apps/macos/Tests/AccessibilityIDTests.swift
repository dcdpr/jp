import Testing

@testable import JP

/// Pins the identifier strings themselves.
///
/// An external driver looks elements up by these names, so they are a contract
/// with something outside this repository: changing one is a breaking change,
/// and these tests are what makes that visible in a diff.
@Suite("AccessibilityID")
struct AccessibilityIDTests {
    @Test("names the sidebar's elements")
    func namesTheSidebar() {
        #expect(AccessibilityID.Sidebar.list == "sidebar.list")
        #expect(AccessibilityID.Sidebar.filter == "sidebar.filter")
        #expect(AccessibilityID.Sidebar.filterClear == "sidebar.filter.clear")
        #expect(AccessibilityID.Sidebar.loadingState == "sidebar.state.loading")
        #expect(AccessibilityID.Sidebar.noMatchesState == "sidebar.state.nomatches")
        #expect(AccessibilityID.Sidebar.unavailableState == "sidebar.state.unavailable")
        #expect(AccessibilityID.Sidebar.row("17251488000") == "sidebar.row.17251488000")
    }

    @Test("names the transcript's elements")
    func namesTheTranscript() {
        #expect(AccessibilityID.Transcript.scroll == "transcript.scroll")
        #expect(AccessibilityID.Transcript.loadingState == "transcript.state.loading")
        #expect(AccessibilityID.Transcript.unavailableState == "transcript.state.unavailable")
        #expect(AccessibilityID.Transcript.text == "transcript.text")
    }

    /// The grab strip between the panes, which a driver reaches by dragging
    /// because the sidebar's width cannot be written through the tree.
    @Test("names the strip that resizes the sidebar")
    func namesThePaneDivider() {
        #expect(AccessibilityID.paneDivider == "window.divider")
    }

    /// A driver that found a row before a title was generated has to still find
    /// it afterwards, so the row's name comes from the conversation ID and
    /// nothing else.
    @Test("names a row the same before and after it is titled")
    func survivesARetitle() {
        let untitled = ConversationSummary(
            id: "17251488000",
            title: nil,
            lastActivatedAt: "2026-08-01T09:00:00Z",
            pinnedAt: nil,
            eventsCount: 4
        )
        let titled = ConversationSummary(
            id: "17251488000",
            title: "Reading list",
            lastActivatedAt: "2026-08-01T09:00:00Z",
            pinnedAt: nil,
            eventsCount: 4
        )

        #expect(
            AccessibilityID.Sidebar.row(untitled.id) == AccessibilityID.Sidebar.row(titled.id))
    }
}
