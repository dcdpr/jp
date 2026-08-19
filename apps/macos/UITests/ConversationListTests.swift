import Testing
import XCTest

/// The Conversation list section of `QA.md`, run rather than read.
///
/// The workspace is three conversations with fixed IDs, titles and activity
/// times, so a test can name the row it wants and say where it should sit.
///
/// One app for the whole suite. These tests read the list and move the
/// selection around, which is state the next test can set for itself, so paying
/// a launch and a terminate each to start from a fresh process buys nothing. A
/// test that needs an app nobody has touched says so and launches its own with
/// ``AppUnderTest/launch(against:keepingWindowState:sourceLocation:)``.
extension UISuite {
    @Suite(
        "ConversationList",
        .sharedApp { try ConversationFixtures.make() }
    )
    @MainActor
    struct ConversationListTests {
        /// The suite's app, and the workspace it was launched against.
        var driven: AppUnderTest { SharedAppBox.shared.app }
        var fixture: WorkspaceFixture { SharedAppBox.shared.workspace }

        /// The workspace's directory name and nothing else. The window carries a
        /// title because the Window menu lists it and a driver addresses it by
        /// it, but it says only which workspace the window is on: a subtitle
        /// counting conversations put a strip of chrome above the transcript that
        /// the design does not have.
        @Test("titles the window with the workspace name alone")
        func namesTheWorkspace() {
            #expect(driven.workspaceWindow(fixture).title == fixture.name)
        }

        @Test("orders conversations most recently active first")
        func ordersByActivity() {
            let newest = driven.row(ConversationFixtures.releaseNotes)
            let middle = driven.row(ConversationFixtures.configPipeline)
            let oldest = driven.row(ConversationFixtures.readingList)

            guard
                driven.expectAppears(newest, "the Release notes row"),
                driven.expectAppears(middle, "the Config pipeline row"),
                driven.expectAppears(oldest, "the Reading list row")
            else { return }

            #expect(newest.frame.minY < middle.frame.minY)
            #expect(middle.frame.minY < oldest.frame.minY)
        }

        /// The date the row also shows is deliberately absent from its label: it
        /// is relative for anything active today, so an assertion on it would
        /// pass or fail depending on the minute the suite ran.
        @Test("shows a row's title and event count together")
        func labelsRows() {
            #expect(
                driven.row(ConversationFixtures.releaseNotes).label
                    == "Release notes, \(ConversationFixtures.releaseNotes.eventCountLabel)"
            )
        }

        /// The row going away and coming back is what says the binding behind the
        /// field is live, rather than the field merely showing the letters typed
        /// into it: an accessibility value can be set on a text field without ever
        /// reaching the state the list is drawn from.
        ///
        /// Leaves the box empty again, because the suite shares one app and every
        /// test after this one expects the whole list.
        @Test("narrows the list while filtering, and restores it when cleared")
        func filtersAndClears() {
            let hidden = driven.row(ConversationFixtures.readingList)
            guard
                driven.expectAppears(hidden, "the Reading list row"),
                // Present whether or not there is anything to clear, so it is
                // there to be found before a word has been typed.
                driven.expectAppears(driven.filterClear, "the clear button")
            else { return }

            driven.filter.click()
            driven.filter.typeText("Release")

            guard driven.expectDisappears(hidden, "the Reading list row, once filtered")
            else { return }

            driven.filterClear.click()

            driven.expectAppears(hidden, "the Reading list row, once cleared")
        }

        /// The whole transcript, exactly: two messages, each under the name of
        /// whoever said it.
        @Test("selects a row on click, and the transcript follows")
        func clickSelects() {
            driven.row(ConversationFixtures.configPipeline).click()

            driven.expectTranscript(
                Transcripts.configPipeline, "the Config pipeline transcript")
        }

        /// The whole row is the click target, not just the text in it. A row
        /// built as a label with padding around it leaves the padding dead, and
        /// clicking beside a title is what a person does.
        @Test("selects a row clicked in the empty space beside its title")
        func clickBesideTitleSelects() {
            driven.row(ConversationFixtures.readingList)
                .coordinate(withNormalizedOffset: CGVector(dx: 0.75, dy: 0.85))
                .click()

            driven.expectTranscript(Transcripts.readingList, "the Reading list transcript")
        }

        @Test("moves the selection with the arrow keys")
        func arrowKeysMoveSelection() {
            // Start at the top row, so one press down lands on a known one.
            driven.row(ConversationFixtures.releaseNotes).click()
            guard
                driven.expectTranscript(
                    Transcripts.releaseNotes, "the Release notes transcript")
            else { return }

            driven.app.typeKey(.downArrow, modifierFlags: [])

            driven.expectTranscript(
                Transcripts.configPipeline,
                "the Config pipeline transcript, after pressing down"
            )
        }

        @Test("opens a conversation in its own window on double-click")
        func doubleClickOpensAWindow() {
            driven.row(ConversationFixtures.readingList).doubleClick()
            defer { driven.closeWindow(titled: "Reading list") }

            let opened = driven.app.windows["Reading list"]
            guard driven.expectAppears(opened, "a window titled Reading list") else { return }

            // Showing the conversation, not an empty pane.
            driven.expectTranscript(
                Transcripts.readingList,
                "the conversation inside its own window",
                within: opened
            )
        }

        /// A different conversation from the double-click test, so a window that
        /// test failed to close could not make this one pass.
        @Test("opens a conversation in its own window from the context menu")
        func contextMenuOpensAWindow() {
            driven.row(ConversationFixtures.releaseNotes).rightClick()
            driven.chooseContextMenuItem("Open in New Window")
            defer { driven.closeWindow(titled: "Release notes") }

            let opened = driven.app.windows["Release notes"]
            guard driven.expectAppears(opened, "a window titled Release notes") else { return }

            driven.expectTranscript(
                Transcripts.releaseNotes,
                "the conversation inside its own window",
                within: opened
            )
        }

        /// The URI lands on a pasteboard of the fixture's own, never the system
        /// one — the app under test is told which to use, and
        /// `ClipboardPolicyTests` holds the suite to it.
        @Test("copies a conversation's URI from the context menu")
        func contextMenuCopiesTheURI() {
            driven.row(ConversationFixtures.readingList).rightClick()
            driven.chooseContextMenuItem("Copy Link")

            #expect(fixture.copiedText() == ConversationFixtures.readingList.uri)
        }

        /// Edit ▸ Copy Link acts on the sidebar selection, so it is greyed out
        /// until there is one — and Escape is how a window gets back to having
        /// none.
        ///
        /// The empty pane is waited for rather than assumed. Without it the
        /// disabled half of this test would also pass against an Escape that did
        /// nothing, in a suite where every earlier test leaves a selection
        /// behind.
        @Test("enables Edit ▸ Copy Link only once a conversation is selected")
        func editCopyLinkFollowsTheSelection() {
            driven.row(ConversationFixtures.configPipeline).click()
            guard
                driven.expectTranscript(
                    Transcripts.configPipeline, "the Config pipeline transcript")
            else { return }

            driven.app.typeKey(.escape, modifierFlags: [])
            guard
                driven.expectDisappears(driven.transcript, "the transcript, after Escape")
            else { return }

            let edit = driven.openMenu("Edit")
            #expect(driven.menuItemIsEnabled("Copy Link", in: edit) == false)
            driven.closeMenu()

            driven.row(ConversationFixtures.configPipeline).click()
            driven.chooseMenuItem("Copy Link", in: "Edit")

            #expect(fixture.copiedText() == ConversationFixtures.configPipeline.uri)
        }

        /// The window holds its two panes itself rather than in a
        /// `NavigationSplitView`, so this item is the app's own and not AppKit's.
        /// Its title flips with what it will do, and it is the only way back to a
        /// hidden sidebar — there is no button for it.
        ///
        /// Leaves the sidebar showing, because the suite shares one app and every
        /// other test addresses a row.
        @Test("hides and shows the sidebar from the View menu")
        func viewMenuTogglesTheSidebar() {
            guard driven.expectAppears(driven.sidebar, "the conversation list") else { return }

            driven.chooseMenuItem("Hide Sidebar", in: "View")
            guard
                driven.expectDisappears(driven.sidebar, "the conversation list, once hidden")
            else { return }

            driven.chooseMenuItem("Show Sidebar", in: "View")
            driven.expectAppears(driven.sidebar, "the conversation list, brought back")
        }

        /// Enter Full Screen and Merge All Windows are items AppKit injects into
        /// menus SwiftUI builds from its own commands. A menu bar rebuilt at the
        /// wrong moment — which a focused value that never compares equal to
        /// itself causes, on every render — drops them.
        @Test("keeps the AppKit-injected View and Window items after selecting")
        func selectingKeepsTheInjectedMenuItems() {
            driven.row(ConversationFixtures.releaseNotes).click()

            let view = driven.openMenu("View")
            #expect(driven.menuItemExists("Enter Full Screen", in: view))
            driven.closeMenu()

            let window = driven.openMenu("Window")
            #expect(driven.menuItemExists("Merge All Windows", in: window))
            driven.closeMenu()
        }
    }
}
