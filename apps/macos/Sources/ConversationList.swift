import SwiftUI

/// The conversation list, as a view of its own so that resizing the sidebar does
/// not re-render it.
///
/// A `List` of a thousand rows is expensive to evaluate, and dragging the divider
/// changes the sidebar's width on every frame of the drag. Built inline in the
/// window's body, the whole list was rebuilt each of those frames and the drag
/// felt heavy. As a separate view compared by its data, SwiftUI finds its inputs
/// unchanged and skips it: the width applies to the frame around it, which costs
/// nothing.
///
/// Equality is by data alone, ignoring the closures and the binding. Those never
/// compare equal, and a value carrying them would differ on every comparison —
/// which is the thing this exists to prevent. ``WorkspaceActions`` makes the same
/// trade for the same reason.
struct ConversationList: View, Equatable {
    /// The conversations to show, in the order they appear.
    let matches: [ConversationSummary]

    /// The rows that draw no line under them.
    let separatorless: Set<ConversationSummary.ID>

    /// The instant the rows date their conversations against.
    ///
    /// Passed in rather than read here, because a fresh `Date()` per render would
    /// make every comparison unequal and defeat the skipping this view is for.
    let now: Date

    /// Which conversation is selected.
    ///
    /// The same value the binding below carries, held separately because equality
    /// has to see it: a `Binding` is read through a closure, which a `nonisolated`
    /// comparison cannot do, and a comparison that ignored the selection would
    /// leave the highlight on the row it was last drawn on.
    let selectedID: ConversationSummary.ID?

    /// The selected conversation, for the list to write as it is clicked through.
    @Binding var selection: ConversationSummary.ID?

    /// The `jp://` reference for a conversation, for dragging it out.
    let reference: (ConversationSummary) -> ConversationRef

    /// Open each named conversation in a window of its own.
    let openWindows: (Set<ConversationSummary.ID>) -> Void

    /// Put each named conversation's URI on the pasteboard.
    let copyLinks: (Set<ConversationSummary.ID>) -> Void

    /// Called when the list leaves the top of its content, or returns to it.
    let scrolledAwayFromTop: (Bool) -> Void

    /// `nonisolated` because `Equatable` is: a `View` is main-actor isolated and its
    /// members inherit that, which a protocol requirement declared without
    /// isolation cannot satisfy. Safe, because everything compared is plain data.
    nonisolated static func == (lhs: Self, rhs: Self) -> Bool {
        return lhs.matches == rhs.matches
            && lhs.separatorless == rhs.separatorless
            && lhs.now == rhs.now
            && lhs.selectedID == rhs.selectedID
    }

    var body: some View {
        List(matches, selection: $selection) { conversation in
            ConversationRow(
                conversation: conversation,
                isSelected: selectedID == conversation.id,
                drawsSeparator: !separatorless.contains(conversation.id),
                now: now
            )
            .draggable(reference(conversation))
            // Silences the table view's own selection fill, which is the system
            // accent colour. Placed in the row because that is where it can reach
            // the table view; see ``ListSelectionHighlight``.
            .background(ListSelectionHighlight.removed)
            // The row fills its cell edge to edge and draws its own background,
            // selection and separator. Everything the list would otherwise
            // contribute is turned off here: its separators run to both edges, and
            // its selection is the system accent colour.
            //
            // Asking for no insets does not get none. A plain list keeps eight
            // points at the leading edge whatever this says, which is the gap a
            // reader sees beside the selection.
            .listRowInsets(EdgeInsets())
            .listRowSeparator(.hidden)
            .listRowBackground(Color.clear)
        }
        .listStyle(.plain)
        // Mapped to a `Bool` rather than watched as an offset: the action then runs
        // when the answer changes rather than on every frame of a scroll.
        .onScrollGeometryChange(for: Bool.self) { geometry in
            geometry.contentOffset.y > 0
        } action: { _, scrolled in
            scrolledAwayFromTop(scrolled)
        }
        // Uncovers the `.background` below. Without it the list draws the system's
        // own list background over the sidebar's colour.
        .scrollContentBackground(.hidden)
        .background(Theme.sidebarBackground.color)
        .accessibilityLabel("Conversations")
        .accessibilityIdentifier(AccessibilityID.Sidebar.list)
        // Escape clears the selection and empties the detail pane. Reaching a
        // workspace with nothing selected is otherwise only possible by opening
        // one, which makes "no conversation chosen" a state the app can enter and
        // never return to.
        //
        // On the list rather than on the window, so Escape in the filter field
        // still means "clear what I typed".
        .onExitCommand { selection = nil }
        // One menu for the list rather than one per row. A per-row `contextMenu` is
        // built for every row the list realizes, which a sidebar of a thousand
        // conversations pays for on every scroll.
        //
        // `primaryAction` is also how a double-click is meant to be handled here: a
        // tap gesture on a row competes with the click the list uses to move the
        // selection.
        //
        // Both act on the full list, not the visible one: an identifier that came
        // from a row is valid whether or not the filter still shows it.
        .contextMenu(forSelectionType: ConversationSummary.ID.self) { ids in
            // These carry no accessibility identifier because they cannot. SwiftUI
            // bridges a menu button to an `NSMenuItem` and does not carry the
            // modifier across, on the button or on its label, so both items report
            // the selector name `menuAction:`. A driver addresses them by title.
            Button("Open in New Window") { openWindows(ids) }
            Divider()
            Button("Copy Link") { copyLinks(ids) }
        } primaryAction: { ids in
            openWindows(ids)
        }
    }
}
