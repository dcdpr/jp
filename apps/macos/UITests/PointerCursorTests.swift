import AppKit
import Foundation
import Testing
import XCTest

/// What the pointer becomes over the things that respond to it.
///
/// The only test in this project that can see a cursor. A cursor is not in the
/// accessibility tree and is not composited into a screenshot, so nothing inside
/// the app can prove one was delivered — `ResizeCursorAreaTests` asserts the view
/// *asks* for a cursor and stayed green through two states where the pointer
/// never changed, which is exactly the gap this closes.
///
/// `NSCursor.currentSystem` reads what the window server is displaying rather
/// than what the calling process requested, so this test process can read the
/// cursor the app under test caused.
///
/// Its own app: it moves the pointer around and leaves it wherever the last hover
/// put it, which is not a state to hand the next suite.
extension UISuite {
    @Suite("PointerCursor")
    @MainActor
    struct PointerCursorTests {
        /// The pointer becomes the horizontal-resize cursor over the strip that
        /// resizes the sidebar.
        ///
        /// Dragging that strip works, and the view asks for the right cursor over
        /// the right area, and the pointer still does not change — the request is
        /// made and not delivered. Until this passes, that is unfixed.
        @Test("shows the horizontal-resize cursor over the pane divider")
        func showsResizeCursorOverTheDivider() {
            let fixture = try? ConversationFixtures.make()
            guard let fixture else {
                Issue.record("could not build the fixture workspace")
                return
            }
            defer { fixture.remove() }

            let driven = AppUnderTest.launch(against: fixture)
            defer { driven.terminate() }

            guard driven.expectAppears(driven.divider, "the pane divider") else { return }

            // The baseline, and it is not optional. `NSCursor.currentSystem` reads
            // the cursor for the whole machine, so a column-resize cursor left
            // showing by anything at all would pass the assertion below without
            // this app having done a thing. Establishing the arrow first turns "it
            // is the right cursor" into "it changed to the right cursor".
            //
            // The conversation list rather than the transcript: the transcript does
            // not exist until something is selected, and hovering a missing element
            // fails without stopping the test — which is how an earlier version of
            // this passed with no baseline at all.
            driven.sidebar.hover()
            let arrowFirst = driven.waitForCursor(.arrow)

            #expect(
                arrowFirst,
                """
                over the conversation list the pointer was \(driven.describeCursor()) \
                rather than the arrow, so this run cannot say whether the divider \
                changed anything.
                """
            )
            guard arrowFirst else { return }

            driven.divider.hover()

            // Read into a `Bool` first: swift-testing reports the expression it
            // evaluated, and `driven` holds an `XCUIApplication` whose description
            // is the entire element tree.
            let changed = driven.waitForCursor(.columnResize)

            #expect(
                changed,
                """
                the pointer over the pane divider did not become the column-resize \
                cursor. It stayed \(driven.describeCursor()). The strip drags \
                correctly, so the gesture reaches it and the pointer does not.
                """
            )
        }
    }
}
