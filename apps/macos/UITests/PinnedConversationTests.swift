import Testing
import XCTest

/// Pinning, end to end: a `pinned_at` timestamp on disk, through the library and
/// its C ABI, to a row that sits at the top of the list and says so.
///
/// Its own app and its own workspace, unlike the rest of the list tests. The
/// shared fixture has no pins, and pinning one of its three conversations would
/// move the row that every ordering assertion in `ConversationListTests` names.
extension UISuite {
    @Suite("PinnedConversations")
    @MainActor
    struct PinnedConversationTests {
        @Test("lifts a pinned conversation above a more recently active one")
        func pinnedSortsFirst() throws {
            let fixture = try ConversationFixtures.makeWithPinnedOldest()
            defer { fixture.remove() }

            let driven = AppUnderTest.launch(against: fixture)
            defer { driven.terminate() }

            let pinned = driven.row(ConversationFixtures.pinnedReadingList)
            let newest = driven.row(ConversationFixtures.releaseNotes)

            guard
                driven.expectAppears(pinned, "the pinned Reading list row"),
                driven.expectAppears(newest, "the Release notes row")
            else { return }

            // Reading list is the oldest of the three, so without the pin it sits
            // below both others; this is the pin moving it and nothing else.
            #expect(pinned.frame.minY < newest.frame.minY)

            // And the row says it is pinned, which is the only way anything
            // outside the app can tell the pin glyph is drawn.
            #expect(
                pinned.label
                    == "Reading list, \(ConversationFixtures.pinnedReadingList.eventCountLabel), pinned"
            )
        }
    }
}
