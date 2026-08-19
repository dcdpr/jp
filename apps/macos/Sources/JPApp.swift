import SwiftUI

/// A reader for JP conversations.
///
/// A plain `WindowGroup`, deliberately: keying the group by workspace path made
/// each window's identity its workspace, which meant ⌘N on a workspace already on
/// screen brought that window forward instead of opening one, and ⌘T had nothing
/// to duplicate. Each window now decides which workspace it shows, and holds that
/// choice in its own scene storage.
@main
struct JPApp: App {
    @State private var recents = RecentWorkspaces()

    init() {
        // Earliest point the app can report which process it is, for a harness
        // that launched it through `open(1)` and got no pid back.
        DebugState.recordProcessID()

        // Also the earliest point it can time itself from, which is what makes
        // "launch to first window" a number rather than an impression.
        Trace.beginLaunch()
    }

    /// What the front window offers the File menu.
    ///
    /// A menu command acts on the focused window, and only that window knows
    /// which workspace it is showing.
    ///
    /// Whatever is published here must compare equal to itself between renders.
    /// See ``WorkspaceActions`` for what happens when it does not.
    @FocusedValue(\.workspaceActions) private var actions

    @Environment(\.openWindow) private var openWindow

    /// The scene identifier `openWindow` addresses workspace windows by.
    private static let workspaceSceneID = "workspace"

    var body: some Scene {
        WindowGroup(id: Self.workspaceSceneID) {
            WorkspaceWindow()
                .environment(recents)
        }
        // No title bar, so no strip of chrome above the transcript and no title
        // text repeating what the sidebar already says. The window buttons stay,
        // over the top-left of the sidebar, and the window still drags by that
        // strip.
        .windowStyle(.hiddenTitleBar)
        .commands { workspaceCommands }

        // A conversation pulled out of a workspace window, into its own.
        WindowGroup(id: ConversationWindow.sceneID, for: ConversationRef.self) { $reference in
            ConversationWindow(reference: reference)
        }
    }

    @CommandsBuilder
    private var workspaceCommands: some Commands {
        // Show/Hide Sidebar, in the View menu where AppKit puts it. Ours rather
        // than `SidebarCommands()`, which acts on a `NavigationSplitView`'s column
        // visibility and the window holds its two panes itself.
        //
        // A hidden sidebar takes the conversation list and the filter box with it,
        // and there is no button for it, so this item and its keystroke are the
        // only way back.
        CommandGroup(after: .sidebar) {
            Button(actions?.isSidebarVisible == false ? "Show Sidebar" : "Hide Sidebar") {
                actions?.toggleSidebar()
            }
            .keyboardShortcut("s", modifiers: [.control, .command])
            .disabled(actions == nil)
        }

        // Replaces "New", which a reader has no use for, but keeps "New Window":
        // macOS hangs window tabbing off it, and without it there is nothing for
        // ⌘T to duplicate.
        CommandGroup(replacing: .newItem) {
            Button("New Window") { openWindow(id: Self.workspaceSceneID) }
                .keyboardShortcut("n", modifiers: .command)

            Divider()

            Button("Open Workspace…") { actions?.choose() }
                .keyboardShortcut("o", modifiers: .command)
                .disabled(actions == nil)

            Menu("Open Recent") {
                ForEach(recents.urls, id: \.self) { url in
                    Button(url.lastPathComponent) { actions?.open(url) }
                }

                if !recents.urls.isEmpty {
                    Divider()
                    Button("Clear Menu") { recents.clear() }
                }
            }
            .disabled(recents.urls.isEmpty || actions == nil)
        }

        // Copying a conversation's URI is the only thing the reader does to a
        // conversation besides opening it, and the list's context menu is a
        // pointing device away. In the Edit menu it also has a keystroke, and it
        // is reachable by anything driving the app through the menu bar.
        CommandGroup(after: .pasteboard) {
            Button("Copy Link") { actions?.copyLinks() }
                .keyboardShortcut("c", modifiers: [.command, .shift])
                .disabled(actions?.hasSelection != true)
        }
    }
}

/// What the focused workspace window lets the File menu do to it.
///
/// Equatable by window, not by content. A focused value is republished every time
/// the view publishing it renders, and the App observing it is invalidated
/// whenever the value differs. Closures never compare equal, so a value carrying
/// them and nothing else differs every single time: the window renders, the App is
/// invalidated, the scene is re-evaluated, the window renders again.
///
/// That loop does not merely rebuild the menu bar — which discards the items
/// AppKit injects into View and Window, since `SwiftUI` reconstructs those menus
/// from its own commands and knows nothing of them. It re-renders the entire scene
/// continuously, and the whole app is sluggish for it: lists stutter as they
/// scroll, and the sidebar snaps rather than animating.
///
/// Comparing the window's identity instead makes a republished value from the same
/// window look unchanged, which ends the loop.
struct WorkspaceActions: Equatable {
    /// Identifies the window these act on, stable for that window's lifetime.
    let windowID: UUID

    /// Whether the window has a conversation selected.
    ///
    /// Part of the equality along with the window, so a menu item conditioned on
    /// it is re-evaluated when the selection appears or goes away, and at no
    /// other time.
    let hasSelection: Bool

    /// Whether the window's sidebar is showing.
    ///
    /// Part of the equality too, because the View menu's item is titled from it.
    let isSidebarVisible: Bool

    /// Put the directory chooser on screen.
    let choose: () -> Void

    /// Show a workspace in this window.
    let open: (URL) -> Void

    /// Put the selected conversation's URI on the pasteboard.
    let copyLinks: () -> Void

    /// Show the sidebar if it is hidden, hide it if it is showing.
    let toggleSidebar: () -> Void

    static func == (lhs: Self, rhs: Self) -> Bool {
        return lhs.windowID == rhs.windowID
            && lhs.hasSelection == rhs.hasSelection
            && lhs.isSidebarVisible == rhs.isSidebarVisible
    }
}

struct WorkspaceActionsKey: FocusedValueKey {
    typealias Value = WorkspaceActions
}

extension FocusedValues {
    var workspaceActions: WorkspaceActions? {
        get { self[WorkspaceActionsKey.self] }
        set { self[WorkspaceActionsKey.self] = newValue }
    }
}
