import Foundation
import Testing

@testable import JP

/// The precedence a window applies when deciding which workspace to show.
/// Pure, so these touch no process state and run in parallel.
@Suite("WorkspaceWindow")
struct WorkspaceWindowTests {
    private let recent = URL(fileURLWithPath: "/workspaces/recent")

    @Test("shows nothing when there is nothing to show")
    func showsNothingWithoutASource() {
        #expect(
            WorkspaceWindow.chooseWorkspace(stored: nil, mostRecent: nil, environment: [:])
                == nil
        )
    }

    @Test("falls back to the most recently opened workspace")
    func fallsBackToTheMostRecent() {
        let chosen = WorkspaceWindow.chooseWorkspace(
            stored: nil,
            mostRecent: recent,
            environment: [:]
        )

        #expect(chosen == "/workspaces/recent")
    }

    /// Two windows on two workspaces is the point of having windows, so a window
    /// that stored a path keeps it rather than following the recents list.
    @Test("prefers the window's own stored path over the recents list")
    func prefersTheStoredPath() {
        let chosen = WorkspaceWindow.chooseWorkspace(
            stored: "/workspaces/stored",
            mostRecent: recent,
            environment: [:]
        )

        #expect(chosen == "/workspaces/stored")
    }

    /// The regression that made `just run-app <workspace>` a no-op after the first
    /// run: once a window had stored a path, the environment was never read again.
    @Test("prefers JP_WORKSPACE over a stored path")
    func prefersTheEnvironmentOverAStoredPath() {
        let chosen = WorkspaceWindow.chooseWorkspace(
            stored: "/workspaces/stored",
            mostRecent: recent,
            environment: ["JP_WORKSPACE": "/workspaces/named"]
        )

        #expect(chosen == "/workspaces/named")
    }

    @Test("prefers JP_WORKSPACE over the recents list")
    func prefersTheEnvironmentOverTheRecentsList() {
        let chosen = WorkspaceWindow.chooseWorkspace(
            stored: nil,
            mostRecent: recent,
            environment: ["JP_WORKSPACE": "/workspaces/named"]
        )

        #expect(chosen == "/workspaces/named")
    }

    /// The app's own scheme sets `JP_WORKSPACE` to an empty string when no
    /// workspace is configured, which must not beat a real stored path.
    @Test("ignores an empty JP_WORKSPACE")
    func ignoresAnEmptyEnvironmentValue() {
        let chosen = WorkspaceWindow.chooseWorkspace(
            stored: "/workspaces/stored",
            mostRecent: recent,
            environment: ["JP_WORKSPACE": ""]
        )

        #expect(chosen == "/workspaces/stored")
    }

    @Test("ignores an empty stored path")
    func ignoresAnEmptyStoredPath() {
        let chosen = WorkspaceWindow.chooseWorkspace(
            stored: "",
            mostRecent: recent,
            environment: [:]
        )

        #expect(chosen == "/workspaces/recent")
    }
}

/// What the focused window offers the menu bar.
///
/// The equality is the whole of it, and it carries more weight than a reader
/// would guess: see ``WorkspaceActions`` for what a value that differs on every
/// render does to the app.
@Suite("WorkspaceActions")
struct WorkspaceActionsTests {
    private func actions(
        windowID: UUID,
        hasSelection: Bool = false,
        isSidebarVisible: Bool = true
    ) -> WorkspaceActions {
        WorkspaceActions(
            windowID: windowID,
            hasSelection: hasSelection,
            isSidebarVisible: isSidebarVisible,
            choose: {},
            open: { _ in },
            copyLinks: {},
            toggleSidebar: {}
        )
    }

    /// The one that matters. A window republishes this on every render with
    /// fresh closures, and closures never compare equal, so comparing them would
    /// invalidate the whole scene continuously.
    @Test("a republished value from the same window compares equal")
    func republishingIsNotAChange() {
        let window = UUID()

        #expect(actions(windowID: window) == actions(windowID: window))
    }

    @Test("a value from another window differs")
    func anotherWindowDiffers() {
        #expect(actions(windowID: UUID()) != actions(windowID: UUID()))
    }

    /// A menu item conditioned on the selection has to be re-evaluated when the
    /// selection appears, and equality is the only thing that asks for it.
    @Test("gaining a selection is a change")
    func gainingASelectionIsAChange() {
        let window = UUID()

        #expect(
            actions(windowID: window, hasSelection: false)
                != actions(windowID: window, hasSelection: true)
        )
    }

    /// The View menu's item is titled from this, so hiding the sidebar has to be
    /// a change or the item keeps saying Hide when it means Show.
    @Test("hiding the sidebar is a change")
    func hidingTheSidebarIsAChange() {
        let window = UUID()

        #expect(
            actions(windowID: window, isSidebarVisible: true)
                != actions(windowID: window, isSidebarVisible: false)
        )
    }
}
