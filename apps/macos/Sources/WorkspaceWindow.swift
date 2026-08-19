import SwiftUI

/// One workspace, in one window.
///
/// Owns its model, so each window reads its own workspace and windows can be
/// tabbed together or pulled apart without sharing state.
struct WorkspaceWindow: View {
    /// The workspace this window shows, restored when the window reopens.
    ///
    /// Per window rather than per app: two windows on two workspaces is the whole
    /// point of having windows.
    @SceneStorage("workspacePath") private var workspacePath: String?

    /// Whether this window's directory chooser is on screen.
    @State private var isChoosingWorkspace = false

    @State private var model = WorkspaceModel()
    @Environment(RecentWorkspaces.self) private var recents

    /// The selected conversation.
    ///
    /// Plain state, mirrored to ``storedSelection`` rather than bound directly to
    /// it: a `List` writes its selection binding while it is handling the click,
    /// and scene storage persists on write, which puts a view update inside the
    /// table view's own update.
    @State private var selection: String?

    /// The selected conversation as the window last had it, restored on reopen.
    @SceneStorage("selectedConversation") private var storedSelection: String?

    /// What the filter box holds.
    ///
    /// Not persisted. A filter is a way of looking at the list right now, and a
    /// window that reopened onto a list mysteriously missing most of its rows
    /// would be a bug report.
    @State private var query = ""

    /// Stable identity for this window, for as long as it exists.
    ///
    /// Only the menu actions use it, and only so a republished
    /// ``WorkspaceActions`` from this window compares equal to the last one.
    @State private var windowID = UUID()

    /// Whether the sidebar is showing.
    ///
    /// Written only when View ▸ Hide Sidebar is chosen, so it is bound straight to
    /// scene storage: a window comes back with the sidebar it was closed with.
    @SceneStorage("sidebarVisible") private var isSidebarVisible = true

    /// How wide the sidebar is.
    ///
    /// Plain state, mirrored to ``storedSidebarWidth`` when a drag ends rather than
    /// bound to it: scene storage persists on every write, and a drag writes on
    /// every frame it is dragged through.
    @State private var sidebarWidth = Self.defaultSidebarWidth

    /// The sidebar's width as the window last had it, restored on reopen.
    @SceneStorage("sidebarWidth") private var storedSidebarWidth: Double?

    /// When the conversations on screen were read.
    ///
    /// What the rows date themselves against. Fixed at the moment of the read
    /// rather than taken fresh per render, because a render happens on every frame
    /// of a divider drag and a clock reading that changes each time would make the
    /// list unequal to itself and undo the skipping that keeps the drag smooth.
    ///
    /// The cost is that "21 minutes ago" is 21 minutes after the workspace was
    /// opened, not after now.
    @State private var listingReadAt = Date()

    /// How tall the search field turned out to be.
    ///
    /// Measured because it follows the field's font rather than a number this view
    /// chooses, and the window buttons are centred against it. Zero until the first
    /// layout, which leaves the buttons where macOS put them.
    @State private var searchFieldHeight: CGFloat = 0

    /// Whether the conversation list is scrolled away from its top.
    ///
    /// Only decides whether a line is drawn above the first row, and only changes
    /// when the list leaves or returns to the top rather than as it scrolls.
    @State private var isListScrolled = false

    /// The width the sidebar was at when the current drag started.
    ///
    /// A drag reports its translation from where it began, so resizing needs the
    /// width it began from. Nil between drags.
    @State private var dragStartWidth: Double?

    @Environment(\.openWindow) private var openWindow

    var body: some View {
        // The whole body, because what it costs is what a window costs to
        // re-render, and the rows underneath are not instrumented: a transcript
        // realizes thousands of them, and a line per row would be a trace nobody
        // can read of a run nobody can time.
        Trace.measuring("WorkspaceWindow.body", target: Self.traceTarget) {
            content
        }
    }

    /// What the window shows, timed by ``body``.
    private var content: some View {
        let listing = self.listing

        return HStack(spacing: 0) {
            if isSidebarVisible {
                sidebar(listing)
                    .frame(width: sidebarWidth)

                splitDivider
                    // Up into the title bar's strip, which the sidebar beside it
                    // already fills. Without this the line starts below the strip
                    // and the window's own title bar colour shows through above it.
                    .ignoresSafeArea(.container, edges: .top)
                    // Above both panes, for hit testing as much as for drawing.
                    //
                    // The grab strip is wider than the line and hangs over the pane
                    // on either side. Later siblings in a stack are in front, so
                    // without this the transcript covers the half of the strip on
                    // its side: approaching the divider from the sidebar worked and
                    // approaching it from the transcript did nothing at all —
                    // neither the cursor nor the drag.
                    .zIndex(1)
            }

            ConversationHistoryView(model: model, conversationID: selection)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        // What keeps the window on screen at all. A window is sized from its
        // content, and content that states only maxima has an ideal size of
        // nothing: the window collapses, and a collapsed window is absent from the
        // window server's list rather than merely small. Held here rather than left
        // to the scene, because the scene's `defaultSize` applies to a window
        // opened fresh and not to one restored into a saved frame.
        .frame(minWidth: Self.minimumWindowWidth, minHeight: Self.minimumWindowHeight)
        // Carried but not displayed: the window has no title bar to show it in.
        // It is still what the Window menu lists the window under, and what an
        // external driver addresses it by.
        .navigationTitle(title)
        // A driven copy of the app can acquire a Space of its own, and a window
        // one Space away is absent from the accessibility tree rather than merely
        // off screen. Nothing outside a debug run.
        .background(DebugSpaces.joinEverySpace())
        // Controls that tint pick this up, which is most of what makes the
        // window look like one app rather than a themed list beside a stock one.
        .tint(Theme.accent.color)
        .onAppear { Trace.endLaunch() }
        .task(id: workspacePath) {
            // Restored before the list exists. Setting it afterwards would
            // change the list's selection during the list's own update.
            selection = storedSelection
            sidebarWidth = storedSidebarWidth ?? Self.defaultSidebarWidth
            await load()
            listingReadAt = Date()
        }
        .onChange(of: selection) { _, new in storedSelection = new }
        // Offers the File menu this window, so ⌘O and Open Recent act on whichever
        // window is in front rather than on the app as a whole.
        .focusedSceneValue(
            \.workspaceActions,
            WorkspaceActions(
                windowID: windowID,
                hasSelection: selection != nil,
                isSidebarVisible: isSidebarVisible,
                choose: { isChoosingWorkspace = true },
                open: { show($0) },
                copyLinks: { copyLinks(for: selectedIDs, among: listing?.all ?? []) },
                toggleSidebar: { isSidebarVisible.toggle() }
            )
        )
        .fileImporter(isPresented: $isChoosingWorkspace, allowedContentTypes: [.folder]) {
            result in
            guard case .success(let url) = result else { return }
            show(url)
        }
    }

    /// The line between the sidebar and the transcript, and the handle that
    /// resizes them.
    ///
    /// Drawn by the window rather than by a `NavigationSplitView`, which was what
    /// held these two panes before. `NSSplitView` draws a translucent divider over
    /// whatever is behind it and offers no way to change either the colour or the
    /// width, so the line came out two pixels of two different greys that shifted
    /// with the content underneath. This one is the colour it is told to be.
    ///
    /// The grab area is wider than the line, because two points is not something a
    /// person can reliably hit.
    private var splitDivider: some View {
        Rectangle()
            .fill(Theme.paneDivider.color)
            .frame(width: Self.dividerWidth)
            .overlay {
                Rectangle()
                    .fill(.clear)
                    .contentShape(.rect)
                    .frame(width: Self.dividerGrabWidth)
                    // SwiftUI's own cursor modifier, and the only thing that works
                    // here. A hosted `NSView` with cursor rects and a hover
                    // callback pushing `NSCursor` both lose the cursor back to an
                    // arrow whenever SwiftUI updates the view — they are competing
                    // with the framework for ownership of it rather than asking.
                    //
                    // `columnResize` is the pointer for a vertical boundary that
                    // moves left and right, which is what this is.
                    .pointerStyle(.columnResize)
                    .gesture(resize)
                    // A clear shape is decorative as far as SwiftUI is concerned
                    // and is left out of the tree entirely, identifier and all.
                    // This is what makes it an element, so a driver can find the
                    // strip and drag it.
                    .accessibilityElement()
                    .accessibilityLabel("Resize sidebar")
                    .accessibilityIdentifier(AccessibilityID.paneDivider)
            }
    }

    /// Widen or narrow the sidebar by dragging the divider.
    private var resize: some Gesture {
        DragGesture(coordinateSpace: .global)
            .onChanged { drag in
                let start = dragStartWidth ?? sidebarWidth
                dragStartWidth = start
                sidebarWidth = min(
                    max(start + drag.translation.width, Self.sidebarWidths.lowerBound),
                    Self.sidebarWidths.upperBound
                )
            }
            .onEnded { _ in
                dragStartWidth = nil
                storedSidebarWidth = sidebarWidth
            }
    }

    /// What the menu commands act on.
    ///
    /// The list's own commands are handed a set by
    /// `contextMenu(forSelectionType:)`, and this is the same thing for the menu
    /// bar, which has no such argument to be given.
    private var selectedIDs: Set<ConversationSummary.ID> {
        selection.map { [$0] } ?? []
    }

    /// Open each named conversation in a window of its own.
    private func openWindows(
        for ids: Set<ConversationSummary.ID>, among all: [ConversationSummary]
    ) {
        for conversation in all where ids.contains(conversation.id) {
            openWindow(id: ConversationWindow.sceneID, value: reference(to: conversation))
        }
    }

    /// Put each named conversation's URI on the pasteboard, one per line.
    private func copyLinks(
        for ids: Set<ConversationSummary.ID>, among all: [ConversationSummary]
    ) {
        let links =
            all
            .filter { ids.contains($0.id) }
            .map { reference(to: $0).uri }
            .joined(separator: "\n")

        guard !links.isEmpty else { return }

        let pasteboard = DebugState.pasteboard
        pasteboard.clearContents()
        pasteboard.setString(links, forType: .string)
    }

    /// Show `url`'s workspace in this window.
    private func show(_ url: URL) {
        selection = nil
        // Canonicalized so a path chosen through the panel and the same path from
        // the recents menu are one value, and reselecting the open workspace does
        // not reload it.
        workspacePath = url.canonicalized.path(percentEncoded: false)
    }

    /// The conversation list as the window is currently showing it.
    private struct Listing {
        /// Everything the workspace holds.
        let all: [ConversationSummary]

        /// What the filter leaves of it, in the order the sidebar shows them.
        let matches: [ConversationSummary]
    }

    /// The listing, once a workspace has been read.
    ///
    /// Derived once per render and handed to both the sidebar and the menu
    /// actions. Filtering walks every conversation and this workspace holds a
    /// thousand, so working it out twice is a thousand extra comparisons on every
    /// keystroke.
    private var listing: Listing? {
        guard case .loaded(let all) = model.state else { return nil }

        return Listing(
            all: all,
            matches: ConversationOrder.pinnedFirst(
                ConversationFilter.matches(all, query: query))
        )
    }

    /// The list, or why there is no list.
    ///
    /// The empty states replace the list rather than covering it, so no table
    /// view exists to be updated while there is nothing to show.
    @ViewBuilder
    private func sidebar(_ listing: Listing?) -> some View {
        switch model.state {
        case .loading:
            ProgressView()
                .controlSize(.small)
                .accessibilityLabel("Loading conversations")
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .accessibilityIdentifier(AccessibilityID.Sidebar.loadingState)

        case .loaded:
            if let listing {
                VStack(spacing: 0) {
                    SearchField(text: $query)
                        // Measured rather than assumed: the field's height follows
                        // its font, and the window buttons are lined up against it.
                        .onGeometryChange(for: CGFloat.self) { proxy in
                            proxy.size.height
                        } action: { height in
                            searchFieldHeight = height
                        }
                        // Moves the window buttons down to the field's own centre.
                        // They sit 14 points down by default, which is the middle
                        // of a title bar this window does not have.
                        .background(
                            WindowButtons.placed(
                                leading: Self.windowButtonsLeading,
                                centredOn: Self.searchFieldPadding + searchFieldHeight / 2
                            )
                        )
                        // Room for the window buttons, which the search field sits
                        // beside rather than below.
                        .padding(.leading, Self.windowButtonsWidth)
                        // The same on the other three sides, so the field sits in
                        // the corner of the window rather than against its edge.
                        .padding([.top, .trailing, .bottom], Self.searchFieldPadding)

                    matchList(listing)
                        // A line above the first row only once the list has
                        // scrolled away from the top, which is what separates the
                        // search field from rows passing under it. At rest there is
                        // nothing to separate, and a line there reads as a border
                        // the design does not have.
                        //
                        // An overlay rather than a row in the stack, so its
                        // appearing does not shift the list down by a point.
                        .overlay(alignment: .top) {
                            if isListScrolled {
                                Rectangle()
                                    .fill(Theme.rowSeparator.color)
                                    .frame(height: 1)
                            }
                        }
                }
                .background(Theme.sidebarBackground.color)
                // Takes the title bar's strip back from the system, which is what
                // puts the search field level with the window buttons instead of
                // under them. Safe because the search field is the topmost thing
                // in the sidebar and it holds its own padding; nothing scrolls
                // under the buttons.
                .ignoresSafeArea(.container, edges: .top)
            }

        case .unavailable(let title, let detail):
            ContentUnavailableView(title, systemImage: "bubble.left", description: Text(detail))
                .accessibilityIdentifier(AccessibilityID.Sidebar.unavailableState)
        }
    }

    /// The conversations matching the filter, or a note that none do.
    ///
    /// The two replace each other rather than one covering the other, for the same
    /// reason as the outer empty states: no table view should exist while there is
    /// nothing for it to show.
    @ViewBuilder
    private func matchList(_ listing: Listing) -> some View {
        if listing.matches.isEmpty {
            ContentUnavailableView.search(text: query)
                .accessibilityIdentifier(AccessibilityID.Sidebar.noMatchesState)
        } else {
            // `.equatable()` rather than left to SwiftUI's own judgement: this is
            // the view whose body must be skipped while the divider is dragged, and
            // the wrapper is what makes the comparison happen for certain.
            ConversationList(
                matches: listing.matches,
                separatorless: ConversationOrder.rowsWithoutSeparator(
                    in: listing.matches, selecting: selection),
                now: listingReadAt,
                selectedID: selection,
                selection: $selection,
                reference: { reference(to: $0) },
                openWindows: { openWindows(for: $0, among: listing.all) },
                copyLinks: { copyLinks(for: $0, among: listing.all) },
                scrolledAwayFromTop: { isListScrolled = $0 }
            )
            .equatable()
        }
    }

    private var title: String {
        workspacePath.map { URL(fileURLWithPath: $0).lastPathComponent } ?? "JP"
    }

    private func reference(to conversation: ConversationSummary) -> ConversationRef {
        ConversationRef(
            workspacePath: workspacePath ?? "",
            conversationID: conversation.id,
            title: conversation.title
        )
    }

    /// How wide the sidebar is in a window that has never been resized.
    private static let defaultSidebarWidth: Double = 280

    /// How narrow and how wide the sidebar can be dragged.
    ///
    /// The lower bound is where a row's title stops being readable; the upper is
    /// where the sidebar starts crowding the transcript.
    private static let sidebarWidths: ClosedRange<Double> = 220...480

    /// The line between the panes.
    ///
    /// One point, which is two pixels on a retina display and matches Bear.
    private static let dividerWidth: CGFloat = 1

    /// The narrowest the window can be.
    ///
    /// The narrowest sidebar, its divider, and enough left over for a line of
    /// transcript to be worth reading.
    private static let minimumWindowWidth: CGFloat =
        CGFloat(sidebarWidths.lowerBound) + dividerWidth + 400

    /// The shortest the window can be: a handful of conversation rows.
    private static let minimumWindowHeight: CGFloat = 400

    /// How much space surrounds the search field on the three sides the window
    /// buttons do not occupy.
    ///
    /// Even padding and buttons level with the field cannot both be had from
    /// layout alone: the buttons sit 14 points down, so an evenly padded field of
    /// height `H` centres at `padding + H/2` and matching 14 forces the field
    /// smaller the more padding it has. The padding is kept even and the buttons
    /// are moved to meet it; see ``WindowButtons``.
    private static let searchFieldPadding: CGFloat = 8

    /// How wide a strip around the divider responds to a drag.
    private static let dividerGrabWidth: CGFloat = 10

    /// Where the first window button's frame is put, from the window's left edge.
    ///
    /// macOS puts it six points in, which reads as cramped against a window with
    /// no title bar. Measured off Bear: the visible circle sits twenty points in,
    /// and the frame is two points wider than the circle on each side.
    private static let windowButtonsLeading: CGFloat = 18

    /// How much of the sidebar's top-left corner the window buttons occupy.
    ///
    /// Three frames at ``WindowButtons/spacing``, from
    /// ``windowButtonsLeading``, and then the gap before the search field starts.
    private static let windowButtonsWidth: CGFloat =
        windowButtonsLeading + 2 * WindowButtons.spacing + 16 + 6

    /// What this window's events are attributed to.
    private static let traceTarget = "JP.Workspace"

    private func load() async {
        guard
            let path = Self.chooseWorkspace(
                stored: workspacePath,
                mostRecent: recents.urls.first,
                environment: ProcessInfo.processInfo.environment
            )
        else { return }

        // Writing this back changes `task(id:)`, which cancels this run and starts
        // another with the chosen path already stored. The second pass chooses the
        // same path and writes nothing.
        if workspacePath != path {
            workspacePath = path
        }

        // Recording the workspace here rather than only where it is chosen keeps
        // a window restored at launch in the recents list too.
        recents.note(URL(fileURLWithPath: path))
        await model.open(path)
    }

    /// The workspace a window should show, in order of precedence.
    ///
    /// 1. `JP_WORKSPACE`, an instruction given at launch.
    /// 2. The path this window stored, so a reopened window comes back where it
    ///    was and two windows can sit on two workspaces.
    /// 3. The most recently opened workspace, which a new window overwhelmingly
    ///    wants and which saves choosing it again.
    ///
    /// The environment comes first because it is the only one of the three a
    /// caller sets deliberately, per launch. Below the stored path it would be
    /// read exactly once in a window's life and silently ignored on every later
    /// launch, which makes `just run-app <workspace>` a no-op after the first run
    /// and leaves a harness unable to point an instance anywhere.
    /// `nonisolated` because it reads none of the view's state. A `View` is
    /// main-actor isolated and its statics inherit that, which this does not need.
    nonisolated static func chooseWorkspace(
        stored: String?,
        mostRecent: URL?,
        environment: [String: String]
    ) -> String? {
        if let named = environment["JP_WORKSPACE"], !named.isEmpty {
            return named
        }

        if let stored, !stored.isEmpty {
            return stored
        }

        return mostRecent?.path(percentEncoded: false)
    }

}
