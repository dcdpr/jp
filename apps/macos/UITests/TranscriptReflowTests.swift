import AppKit
import Foundation
import Testing
import XCTest

/// Whether the transcript re-wraps while a window is being dragged.
///
/// Its own app rather than the shared one: it needs a conversation tall enough
/// to scroll, and it resizes and scrolls the window it is given, which is not a
/// state to hand the next suite.
extension UISuite {
    @Suite("TranscriptReflow")
    @MainActor
    struct TranscriptReflowTests {
        /// The interval the app writes once per window drag.
        private static let drag = "transcript.liveresize"

        /// How far the drag moves the window's right edge, in points.
        ///
        /// Inwards, which needs no room beyond the window itself and so works on
        /// any display that can show the window at all. Growing instead would need
        /// the pointer to travel past the window's right edge, and a pointer stops
        /// at the bounds of the display.
        ///
        /// Small enough to stay above the window's minimum width, which is what
        /// makes the drag deliver frames the whole way rather than stopping short:
        /// ``AppUnderTest/windowFrame`` is 1000 wide against a 621 minimum.
        private static let dragBy: CGFloat = -220

        /// The whole point: text re-wraps on every frame of a window drag, not
        /// once the mouse comes up.
        ///
        /// A window resize reaches the text through the text container, whose width
        /// the text view is supposed to keep in step with its own. AppKit does not
        /// do that while a resize is in progress, so nothing changes the
        /// container's geometry, nothing invalidates layout, and the view redraws
        /// lines wrapped to a width the window no longer has. The app sets the
        /// container's width itself for exactly this reason.
        ///
        /// Asserted through the app's own trace rather than off the screen, because
        /// the defect leaves nothing behind: on mouse-up the container catches up
        /// and the text is correct either way. Only what happened *during* the drag
        /// tells the two apart.
        @Test("re-wraps the transcript during a window drag, scrolled away from the top")
        func reflowsWhileDragging() throws {
            let fixture = try ConversationFixtures.makeLongRead()
            defer { fixture.remove() }

            let driven = AppUnderTest.launch(against: fixture)
            defer { driven.terminate() }

            driven.row(ConversationFixtures.longRead).click()
            guard driven.expectAppears(driven.transcriptText, "the transcript's text")
            else { return }

            scrollToTheEnd(of: driven)
            let dragged = dragTheWindowEdge(of: driven, fixture)

            let record = try #require(
                fixture.lastTracedInterval(named: Self.drag),
                """
                the app traced no window drag, so the gesture never reached it. \
                The window was at \(dragged) and the screen's visible frame is \
                \(NSScreen.main.map { "\($0.visibleFrame)" } ?? "unknown"): a \
                window edge outside that frame cannot be pressed, because the \
                pointer stops at the display bounds.
                """
            )

            // Two preconditions before the assertion, because each of them failing
            // would leave a test that passes without having tried anything.
            #expect(
                (record["visible_from_y"] ?? 0) > 1000,
                """
                the transcript was still near the top of the document, where the \
                defect does not show: \(record)
                """
            )
            #expect(
                (record["width_changes"] ?? 0) > 4,
                """
                the drag delivered almost no width changes, so it was a jump rather \
                than a gesture: \(record)
                """
            )

            // The assertion. Zero is what the defect produces: the view resized
            // hundreds of times and the container was told nothing.
            #expect(
                (record["container_changes"] ?? 0) > 0,
                """
                the text container's width never changed while the window was being \
                dragged, so the text on screen stayed wrapped to the old width \
                until the mouse came up: \(record)
                """
            )
        }

        /// Put the transcript at the end of the document.
        ///
        /// Through the text view's own Command-Down rather than a synthesized
        /// scroll wheel: `scroll(byDeltaX:deltaY:)` reported synthesizing an event
        /// and left the transcript where it was. The end is used rather than a
        /// measured fraction because it is a position the view can be asked for
        /// exactly, and anywhere past the first fifth of the document is equally
        /// good for what is being tested.
        private func scrollToTheEnd(of driven: AppUnderTest) {
            driven.transcriptText.click()
            driven.app.typeKey(.downArrow, modifierFlags: .command)
        }

        /// Drag the window's right edge, once, and report the frame it started
        /// from.
        ///
        /// One gesture, and no attempt to put the window back. A coordinate is
        /// resolved against its element's frame at the moment it is *used*, not
        /// when it is made, so a second gesture written against the same two
        /// coordinates re-resolves both against the window the first one just
        /// moved: the return drag starts inside the window body and pulls a
        /// stretch of empty transcript instead of the edge.
        ///
        /// Nothing needs the width restored, and nothing depends on where the last
        /// run left it: every launch pins the frame.
        ///
        /// The returned frame is what the caller names when no drag was traced,
        /// which is the shape of failure a window edge off the side of the screen
        /// produces.
        ///
        /// The window is raised by the click that preceded this, so the edge is
        /// where the tree says it is.
        private func dragTheWindowEdge(
            of driven: AppUnderTest,
            _ fixture: WorkspaceFixture
        ) -> CGRect {
            let window = driven.workspaceWindow(fixture)
            let before = window.frame
            let edge = window.coordinate(withNormalizedOffset: CGVector(dx: 1, dy: 0.5))

            edge.press(
                forDuration: 0.1,
                thenDragTo: edge.withOffset(CGVector(dx: Self.dragBy, dy: 0))
            )

            return before
        }
    }
}
