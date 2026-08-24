import Foundation
import Testing
import XCTest

/// Stable names for the elements this suite reaches for.
///
/// Deliberately spelled out rather than shared with the app's
/// `AccessibilityID`: these identifiers are the contract an external driver
/// holds the app to, documented in `AFFORDANCES.md`. A suite that imported the
/// constants would follow a rename instead of catching one, and a UI test runs
/// in another process anyway.
///
/// `AccessibilityIDTests` pins the same strings from inside the app.
///
/// Only the names this suite uses are here. The rest of the table stays out
/// until a test drives the state that shows it, so every name in this file is
/// one something depends on.
enum ID {
    static let sidebarList = "sidebar.list"
    static let sidebarFilter = "sidebar.filter"
    static let sidebarFilterClear = "sidebar.filter.clear"
    static let transcriptScroll = "transcript.scroll"
    static let transcriptText = "transcript.text"
    static let windowDivider = "window.divider"

    static func sidebarRow(_ conversationID: String) -> String {
        "sidebar.row.\(conversationID)"
    }

}

/// The app, launched against a fixture and driven from outside its process.
///
/// Isolation is by environment, with one exception the environment cannot
/// reach: window state saved by `@SceneStorage` is keyed by bundle identifier,
/// and a UI test drives the developer's own build under the developer's own
/// identifier. `-ApplePersistenceIgnoreState` is the lever that leaves it
/// alone — the app neither restores what was saved nor saves what it had.
///
/// A test of state restoration is the one case that needs the opposite, and
/// passes `keepingWindowState: true` knowingly.
///
/// ## What a test costs
///
/// A synthesized pointer event — a click, a double-click, a right-click, or
/// opening a menu-bar menu — costs 400-500ms. A key event costs ~50ms and
/// resolving an element ~40ms, so the pointer path is an order of magnitude
/// dearer than anything else a test does, and it dominates the run.
///
/// None of it is the app. Timestamps on both sides put the app's own work 71ms
/// *after* `click()` has already returned, and the work itself at 2-4ms: XCTest
/// spends the 400ms before the event is delivered, so nothing the app does or
/// stops doing changes it. Turning off the post-event idle wait (see
/// ``Quiescence``) buys about 30ms of it and there is no second knob.
///
/// What does move is the size of the accessibility tree, at roughly 0.3ms per
/// element per event. This fixture publishes ~230 elements, which is ~70ms of
/// each click; a fixture of 300 conversations publishes ~1130 and makes every
/// pointer event half again as expensive. Size a fixture for what the test
/// needs to say, not for realism.
///
/// So: prefer a key event to a pointer event wherever the affordance allows,
/// and reach for a pointer event only where the pointer *is* what is under
/// test.
@MainActor
struct AppUnderTest {
    let app: XCUIApplication

    /// How long to wait for the app to read its workspace and draw a list.
    ///
    /// Wider than ``timeout`` because it covers process start, not just work
    /// the running app does.
    static let launchTimeout: TimeInterval = 10

    /// How long to wait for anything the app does once it is up.
    ///
    /// Deliberately short. Every wait here is on a condition rather than on the
    /// clock, so a passing test returns the moment the element appears and this
    /// number costs it nothing — it is the price of a *failure*, paid once per
    /// broken assertion, and ten seconds of that is ten seconds of a red loop
    /// spent watching a spinner.
    ///
    /// One second is far longer than anything the app does in reply to a click:
    /// the workspace is already open by then, and reading a conversation of
    /// four events is a file read. Raise it for a specific wait that genuinely
    /// covers slower work rather than raising it here.
    static let timeout: TimeInterval = 1

    /// The frame every launched app starts at, as `<width>x<height>`.
    ///
    /// Small enough to leave room on any screen worth running tests on, for a
    /// test that drags the window wider. Large enough to show a conversation.
    static let windowFrame = "1000x700"

    /// The variable the app reads that frame from.
    ///
    /// Spelled out rather than referenced: a UI test bundle drives the app from
    /// another process and cannot import it. `FixedWindowFrameTests` in the unit
    /// tests pins the app's own constant to this string, so renaming one without
    /// the other fails there.
    static let windowFrameKey = "JP_WINDOW_FRAME"

    /// Launch against `fixture` and wait until the conversation list is on
    /// screen.
    ///
    /// Waiting here rather than in each test is what keeps a test from acting on
    /// a window that has not finished reading, which reads as an intermittent
    /// failure rather than as the race it is.
    static func launch(
        against fixture: WorkspaceFixture,
        keepingWindowState: Bool = false,
        sourceLocation: SourceLocation = #_sourceLocation
    ) -> AppUnderTest {
        // Before the first event is synthesized, and reported rather than
        // shrugged off: a suite quietly back to waiting after every event is a
        // suite nobody notices has slowed down.
        if let failure = Quiescence.installation {
            let message = "the quiescence waits could not be turned off: \(failure)"
            Diagnostics.append("\(sourceLocation.fileName):\(sourceLocation.line): \(message)")
            Issue.record("\(message)", sourceLocation: sourceLocation)
        }

        let app = XCUIApplication()

        // The menu titles AppKit supplies — Edit, View, Window, Enter Full
        // Screen, Merge All Windows — are localized, and this suite addresses
        // them by their English names. Unpinned, they are whatever the machine
        // running the tests prefers, and every menu test fails on a Mac set to
        // another language.
        app.launchArguments = ["-AppleLanguages", "(en)", "-AppleLocale", "en_US"]

        var environment = fixture.environment
        if !keepingWindowState {
            app.launchArguments += ["-ApplePersistenceIgnoreState", "YES"]

            // Saved state and autosaved window frames are two mechanisms, and the
            // launch argument above only disables the first. Without this the
            // window opens at whatever size the previous run left it, so a suite
            // that resizes hands the next one a different starting point — and one
            // that grows the window eventually hands it a window against the edge
            // of the screen with nowhere left to grow.
            environment[Self.windowFrameKey] = Self.windowFrame
        }

        app.launchEnvironment = environment
        app.launch()

        let driven = AppUnderTest(app: app)
        _ = driven.wait(for: driven.sidebar, timeout: launchTimeout)

        // Recorded once the app is up, so a run stopped part-way can still
        // close it. Nothing else can: the app outlives the process that stops
        // the run. See ``Diagnostics/processes``.
        if let pid = fixture.appProcessID {
            Diagnostics.recordAppProcess(pid)
        }

        return driven
    }

    /// Wait for `element` to exist, asking as often as asking costs.
    ///
    /// `XCUIElement.waitForExistence` reports an element about a second after
    /// it appears, whatever it is: the two transcript waits in this suite
    /// measured 1131ms and 1117ms against an app that draws them in tens of
    /// milliseconds, and the number barely moves with the work involved.
    ///
    /// Resolving an element costs about 30ms, so a loop that simply asks again
    /// polls at roughly 30Hz and finds it an order of magnitude sooner. No
    /// sleep, and none needed: the query is what paces the loop.
    func wait(for element: XCUIElement, timeout: TimeInterval = AppUnderTest.timeout) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)

        repeat {
            if element.exists {
                return true
            }
        } while Date() < deadline

        return false
    }

    func terminate() {
        app.terminate()
    }

    /// The window showing the workspace, addressed by the title it carries.
    ///
    /// By title rather than `windows.firstMatch`, because a test that opened a
    /// conversation window leaves it behind for the next one and first is not
    /// the same as the workspace's.
    func workspaceWindow(_ fixture: WorkspaceFixture) -> XCUIElement {
        app.windows.element(matching: NSPredicate(format: "title BEGINSWITH %@", fixture.name))
    }

    /// Close the window titled `title`, if it is open.
    ///
    /// Tests that open a window close it again, so the next one starts from the
    /// same arrangement it would have found on its own.
    ///
    /// Command-W rather than the close button, because a synthesized pointer
    /// event costs around 400ms and a key event around 50. It acts on whichever
    /// window is in front, which is why the title is checked afterwards instead
    /// of aimed at: a Command-W arriving while the workspace window was in front
    /// would close *that*, and every test after it would fail somewhere far away
    /// from the cause.
    func closeWindow(
        titled title: String,
        sourceLocation: SourceLocation = #_sourceLocation
    ) {
        let window = app.windows[title]
        guard window.exists else { return }

        app.typeKey("w", modifierFlags: .command)

        guard waitForDisappearance(of: window) else {
            record(
                """
                the window titled "\(title)" was still open after Command-W, so \
                the key window was something else and that is what closed. \
                On screen: \(capture("stuck window \(title)"))
                """,
                sourceLocation: sourceLocation
            )
            return
        }
    }

    /// Wait for `element` to stop existing, asking as often as asking costs.
    ///
    /// The counterpart to ``wait(for:timeout:)``, and paced the same way.
    func waitForDisappearance(
        of element: XCUIElement,
        timeout: TimeInterval = AppUnderTest.timeout
    ) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)

        repeat {
            if !element.exists {
                return true
            }
        } while Date() < deadline

        return false
    }

    // Every accessor below names an element type and starts from the narrowest
    // root it can. An untyped `descendants(matching: .any)` reads as convenient
    // and costs a full snapshot of the app's accessibility tree on each
    // evaluation, which is most of what a test spends its time on. The types
    // are what the app actually publishes, read off a running instance with
    // `debug_app_snapshot`.

    /// The conversation list, which exists only once the workspace is read.
    ///
    /// A SwiftUI `List` in a sidebar is an `NSOutlineView`.
    var sidebar: XCUIElement {
        app.outlines[ID.sidebarList]
    }

    /// The box that narrows the conversation list.
    var filter: XCUIElement {
        app.textFields[ID.sidebarFilter]
    }

    /// The button that empties the filter box, which exists only while the box
    /// holds something.
    var filterClear: XCUIElement {
        app.buttons[ID.sidebarFilterClear]
    }

    /// The scrolling transcript, which exists only once a conversation is read.
    var transcript: XCUIElement {
        app.scrollViews[ID.transcriptScroll]
    }

    /// The row showing `conversation`.
    ///
    /// A row's identifier sits on the leaf inside its cell rather than on the
    /// row, because the view carrying it collapses to one element. That leaf
    /// reports no role of its own, which is why this asks for `.other` rather
    /// than for a cell or a row.
    func row(_ conversation: FixtureConversation) -> XCUIElement {
        sidebar.descendants(matching: .other)[ID.sidebarRow(conversation.id)]
    }

    /// The strip between the panes that resizes the sidebar.
    ///
    /// `.any` rather than a role, because the view reports none of its own: it is
    /// a shape made into an accessibility element, and arrives as `AXUnknown`.
    var divider: XCUIElement {
        app.descendants(matching: .any)[ID.windowDivider]
    }

    /// Wait until the system is displaying `cursor`.
    ///
    /// `NSCursor.currentSystem` reads what the window server is showing rather
    /// than what this process asked for, so a test in another process can see the
    /// cursor the app under test caused. Compared by image bytes: the accessor
    /// hands back a fresh instance each time, so identity says nothing, and two
    /// standard cursors differ in their pixels.
    ///
    /// Polled rather than read once, because the window server sets the cursor a
    /// moment after the pointer arrives.
    func waitForCursor(
        _ cursor: NSCursor,
        timeout: TimeInterval = AppUnderTest.timeout
    ) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)

        repeat {
            if cursorIs(cursor) {
                return true
            }
        } while Date() < deadline

        return false
    }

    /// Whether the system is showing `cursor` right now.
    ///
    /// Compared by image bytes, and only ever used to ask about a cursor the test
    /// is looking *for*. Not every standard cursor can be recognised this way —
    /// `NSCursor.arrow.image` does not match the bytes the system reports while
    /// showing the arrow — so a test that needs a baseline asks whether the
    /// cursor is *not* the one it expects next, rather than trying to name what it
    /// currently is.
    func cursorIs(_ cursor: NSCursor) -> Bool {
        guard let current = NSCursor.currentSystem?.image.tiffRepresentation else {
            return false
        }

        return current == cursor.image.tiffRepresentation
    }

    /// The cursor the system is showing, named against the standard ones.
    ///
    /// For a failure message: an `NSCursor`'s own description is a pointer
    /// address, which says only that it was not the expected one.
    func describeCursor() -> String {
        guard let current = NSCursor.currentSystem?.image.tiffRepresentation else {
            return "a cursor the system would not report"
        }

        let known: [(String, NSCursor)] = [
            ("the arrow", .arrow),
            ("the I-beam", .iBeam),
            ("the pointing hand", .pointingHand),
            ("the open hand", .openHand),
            ("the column-resize cursor", .columnResize),
            ("the row-resize cursor", .rowResize),
            ("the left-right resize cursor", .resizeLeftRight),
        ]

        let match = known.first { $0.1.image.tiffRepresentation == current }
        return match?.0 ?? "a cursor matching none of the standard ones"
    }

    /// The text the transcript is drawn as.
    ///
    /// The whole conversation is one text view, so there is no element per
    /// message. Its value is every message it is showing, which is how a test
    /// asserts on what is on screen.
    var transcriptText: XCUIElement {
        app.textViews[ID.transcriptText]
    }

    /// Wait until a transcript shows exactly `text`.
    ///
    /// Exactly, and against the whole document rather than a phrase inside it: the
    /// value of the text view is every message it is showing, so a substring match
    /// would survive the speaker labels going missing, the messages arriving in the
    /// wrong order, or a second copy of the conversation being appended.
    ///
    /// `within` scopes the search to one window, which is how a conversation pulled
    /// into its own window is told apart from the workspace window behind it. The
    /// identifier is the same in both.
    ///
    /// Polled rather than read once, because a transcript arrives a moment after
    /// the row is clicked.
    @discardableResult
    func expectTranscript(
        _ text: String,
        _ description: String,
        within scope: XCUIElement? = nil,
        timeout: TimeInterval = AppUnderTest.timeout,
        sourceLocation: SourceLocation = #_sourceLocation
    ) -> Bool {
        let element = (scope ?? app).textViews[ID.transcriptText]
        let deadline = Date().addingTimeInterval(timeout)
        var last: String?

        repeat {
            last = element.exists ? element.value as? String : nil
            if last == text {
                return true
            }
        } while Date() < deadline

        let shot = capture(description)
        record(
            """
            \(description) never showed the expected transcript within \(timeout)s. \
            Showing instead: \(last.map { "\($0.debugDescription)" } ?? "no transcript at all"). \
            On screen: tmp/uitests/\(shot)
            """,
            sourceLocation: sourceLocation
        )
        return false
    }

    /// Open the menu-bar menu titled `title` and return the menu it drops down,
    /// so its items can be read.
    ///
    /// The dropped-down menu rather than the bar item, because it is the root
    /// every item below is addressed from. A title is not unique across the
    /// app: Copy Link is both an Edit-menu item and a context-menu item, and
    /// AppKit publishes both to the accessibility tree whether or not either
    /// menu is open, so a query starting at the app can return the wrong one.
    /// Starting at the menu cannot.
    ///
    /// macOS populates a menu when it is opened, so an item's presence and
    /// enablement still cannot be read from a closed one.
    @discardableResult
    func openMenu(_ title: String) -> XCUIElement {
        let bar = app.menuBars.menuBarItems[title]
        _ = wait(for: bar)
        bar.click()
        return bar.menus.firstMatch
    }

    /// Close whatever menu is open, by pressing Escape.
    func closeMenu() {
        app.typeKey(.escape, modifierFlags: [])
    }

    /// Open the menu-bar menu `menu` and click `item` in it.
    func chooseMenuItem(
        _ item: String,
        in menu: String,
        sourceLocation: SourceLocation = #_sourceLocation
    ) {
        let entry = openMenu(menu).menuItems[item]
        guard
            expectAppears(entry, "\(item) in the \(menu) menu", sourceLocation: sourceLocation)
        else { return }

        entry.click()
    }

    /// Click `item` in the context menu that is open.
    ///
    /// A context menu has no handle to start from the way a menu-bar menu does,
    /// so this picks between same-titled items by hittability: only the items
    /// of an open menu are hittable, and the context menu is the one that is
    /// open. Getting it wrong is worth avoiding rather than merely detecting —
    /// the menu-bar twin of a context item is usually disabled, so the click
    /// lands and silently does nothing, which looks exactly like the app
    /// ignoring the menu.
    func chooseContextMenuItem(
        _ item: String,
        sourceLocation: SourceLocation = #_sourceLocation
    ) {
        let matches = app.menuItems.matching(identifier: item)
        _ = wait(for: matches.firstMatch)

        guard let entry = matches.allElementsBoundByIndex.first(where: \.isHittable) else {
            record(
                """
                no open menu holds an item titled "\(item)": \
                \(matches.count) match it, none of them on screen. \
                On screen instead: \(capture(item))
                """,
                sourceLocation: sourceLocation
            )
            return
        }

        entry.click()
    }

    /// Whether `menu` holds an item titled `item`.
    ///
    /// Presence, not enablement: an item AppKit injects can be there and greyed
    /// out — Merge All Windows is, until there is a second window to merge —
    /// and it is the presence that says the menu was not rebuilt out from under
    /// it.
    func menuItemExists(_ item: String, in menu: XCUIElement) -> Bool {
        menu.menuItems[item].exists
    }

    /// Whether `menu`'s `item` can be chosen.
    func menuItemIsEnabled(_ item: String, in menu: XCUIElement) -> Bool {
        let entry = menu.menuItems[item]
        return entry.exists && entry.isEnabled
    }

    /// Wait for `element`, recording what was on screen instead when it never
    /// arrives.
    ///
    /// A bare `#expect(element.exists)` reports only that something was
    /// missing, which is the least useful half of the story: the app was
    /// showing *something*, and what it was showing is usually the whole
    /// answer. This writes that screen to a PNG and names the file in the
    /// failure.
    ///
    /// The path rather than the image, because a tool result is text all the
    /// way to the assistant reading it. Attach the file to say what it shows.
    @discardableResult
    func expectAppears(
        _ element: XCUIElement,
        _ description: String,
        timeout: TimeInterval = AppUnderTest.timeout,
        sourceLocation: SourceLocation = #_sourceLocation
    ) -> Bool {
        if wait(for: element, timeout: timeout) {
            return true
        }

        let shot = capture(description)
        record(
            "\(description) never appeared within \(timeout)s. On screen instead: tmp/uitests/\(shot)",
            sourceLocation: sourceLocation
        )
        return false
    }

    /// Wait for `element` to go away, recording what is still on screen when it
    /// does not.
    ///
    /// The counterpart to ``expectAppears(_:_:timeout:sourceLocation:)``, for
    /// the assertions that say something was torn down rather than built.
    @discardableResult
    func expectDisappears(
        _ element: XCUIElement,
        _ description: String,
        timeout: TimeInterval = AppUnderTest.timeout,
        sourceLocation: SourceLocation = #_sourceLocation
    ) -> Bool {
        if waitForDisappearance(of: element, timeout: timeout) {
            return true
        }

        let shot = capture(description)
        record(
            "\(description) never went away within \(timeout)s. On screen: tmp/uitests/\(shot)",
            sourceLocation: sourceLocation
        )
        return false
    }

    /// Wait until `fixture`'s pasteboard holds `text`.
    ///
    /// Polled rather than read once. The quiescence waits are off, so a
    /// synthesized click returns before the app has finished handling it —
    /// measured at 71ms of app work after `click()` had already come back — and
    /// reading the pasteboard on the next statement races that work.
    ///
    /// Slept between reads, unlike the element waits: a pasteboard read is cheap
    /// enough to spin thousands of times a second, and the loop would compete
    /// with the app for the core it is waiting on.
    @discardableResult
    func expectCopied(
        _ text: String,
        to fixture: WorkspaceFixture,
        timeout: TimeInterval = AppUnderTest.timeout,
        sourceLocation: SourceLocation = #_sourceLocation
    ) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        var last: String?

        repeat {
            last = fixture.copiedText()
            if last == text {
                return true
            }
            Thread.sleep(forTimeInterval: 0.01)
        } while Date() < deadline

        record(
            """
            the pasteboard never held \(text.debugDescription) within \(timeout)s. \
            It held \(last.map { $0.debugDescription } ?? "nothing").
            """,
            sourceLocation: sourceLocation
        )
        return false
    }

    /// Record a failure, in both places a reader might look.
    ///
    /// `Issue.record` alone is not enough under `xcodebuild`, which prints the
    /// header naming the *kind* of issue and drops the message explaining it:
    /// a run of ten failures arrives as ten identical `Issue recorded` lines.
    /// So the message also goes to a file `swift_test_ui` collects. That is
    /// also what lets the tool stop the run: it watches for the failure, and
    /// the message it reports afterwards comes from here rather than from
    /// output that was cut off mid-write.
    func record(_ message: String, sourceLocation: SourceLocation) {
        Diagnostics.append("\(sourceLocation.fileName):\(sourceLocation.line): \(message)")
        Issue.record("\(message)", sourceLocation: sourceLocation)
    }

    /// Write what the app is showing to a PNG, and return its file name.
    ///
    /// The name rather than the path: the file is written into the runner's
    /// container, and `swift_test_ui` copies it into `tmp/uitests/` under the
    /// same name. Naming the container path here would give a reader a path
    /// that is longer and gone by the next run.
    ///
    /// Returns why it could not be written rather than throwing, because this
    /// runs while a test is already failing and a second failure would bury the
    /// first.
    func capture(_ description: String) -> String {
        let name =
            description
            .replacingOccurrences(of: "/", with: "-")
            .replacingOccurrences(of: " ", with: "-")
            .prefix(80)
        let file = Diagnostics.directory
            .appendingPathComponent("\(name)-\(UUID().uuidString.prefix(8)).png")

        do {
            try FileManager.default.createDirectory(
                at: Diagnostics.directory,
                withIntermediateDirectories: true
            )
            try app.screenshot().pngRepresentation.write(to: file)
        } catch {
            return "(no screenshot: \(error))"
        }

        return file.lastPathComponent
    }

}
