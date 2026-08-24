import SwiftUI

/// One conversation in the sidebar.
struct ConversationRow: View {
    /// The height every row is laid out at.
    ///
    /// Fixed, not measured. A list has to know its total content height to size
    /// its scroll bar, and with variable-height rows that means measuring every
    /// row rather than the visible ones — a cost that grows with the number of
    /// conversations. A uniform height lets it multiply instead.
    ///
    /// Sized for two lines of title over one of metadata, which is the tallest a
    /// row gets. A title of one line leaves the rest of the space empty rather
    /// than closing the gap, so the metadata sits on the same baseline in every
    /// row. A larger system text size would clip it; a row that grows with the
    /// text needs the list to supply the height some other way.
    static let height: CGFloat = 72

    /// How far the text sits in from the row's own leading edge.
    private static let textInset: CGFloat = 16

    /// How wide the bar marking the selected row is.
    private static let accentBarWidth: CGFloat = 5

    private static let selectionRadius: CGFloat = 6

    let conversation: ConversationSummary

    /// Whether this is the selected conversation.
    ///
    /// The row draws its own selection rather than letting the list draw one; see
    /// ``ListSelectionHighlight`` for why it has to.
    let isSelected: Bool

    /// Whether to draw the line under the row.
    ///
    /// False for the selected row and the one above it, so no separator cuts
    /// across either end of the selection's rounded fill.
    let drawsSeparator: Bool

    /// The instant the row dates the conversation against.
    ///
    /// Passed in rather than read here, so one clock read covers a whole render
    /// of the list instead of one per realized row.
    let now: Date

    var body: some View {
        ZStack {
            Theme.sidebarBackground.color

            if isSelected {
                selection
            }

            text
        }
        .frame(height: Self.height)
        .overlay(alignment: .bottom) {
            if drawsSeparator {
                separator
            }
        }
        // Labelled explicitly, and children ignored rather than combined:
        // combining walks and merges each row's accessibility subtree, which a
        // sidebar of a thousand rows pays for as it scrolls.
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(label)
        // Safe on the same view as the `.ignore` above: that collapses the row
        // to one leaf element, and this names it. A row is addressed by the
        // conversation's ID, so retitling one does not move it.
        .accessibilityIdentifier(AccessibilityID.Sidebar.row(conversation.id))
    }

    /// The title, and the metadata under it.
    private var text: some View {
        // No spacing and no spacer between the two. A `Spacer` here is charged the
        // stack's spacing twice, once on each side of it, and those eight points
        // are the difference between a row that fits two lines of title and one
        // that fits one: the title then takes a single line and truncates however
        // high its line limit is. The title claims the leftover height instead,
        // which holds the metadata to the bottom just as well.
        VStack(alignment: .leading, spacing: 0) {
            Text(verbatim: title)
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(Theme.bodyText.color)
                .lineLimit(2)
                .frame(maxHeight: .infinity, alignment: .topLeading)

            metadata
        }
        // Inside the padding, not around it. Outside, the stack keeps its ideal
        // width and the title is offered as much room as it asks for: it then
        // never wraps, and the row clips it into an ellipsis instead. Inside, the
        // stack is handed the row's width and a long title wraps to its second
        // line as intended.
        .frame(maxWidth: .infinity, alignment: .topLeading)
        .padding(.horizontal, Self.textInset)
        .padding(.vertical, 10)
    }

    /// The pin, the date and the event count, under the title.
    private var metadata: some View {
        HStack(spacing: 5) {
            if conversation.isPinned {
                // Rotated because SF Symbols draws a pin upright and this one
                // reads as pinning something to a board.
                Image(systemName: "pin.fill")
                    .rotationEffect(.degrees(45))
                    .foregroundStyle(Theme.accent.color)
            }

            if let date = ConversationDate.activityLabel(for: conversation, now: now) {
                Text(verbatim: date)
                Text(verbatim: "·")
            }

            Text(verbatim: eventCount)
        }
        .font(.system(size: 11))
        .foregroundStyle(Theme.secondaryText.color)
    }

    /// The line under the row.
    ///
    /// Drawn by the row rather than by the list, for two reasons:
    /// `listRowSeparatorTint` leaves a plain list's separators the system colour
    /// on macOS, and the list draws them edge to edge.
    private var separator: some View {
        Rectangle()
            .fill(Theme.rowSeparator.color)
            .frame(height: 1)
    }

    /// What fills the selected row.
    ///
    /// The accent bar belongs to the fill rather than to the row, and is clipped
    /// to the same rounded rectangle: against the row's edge it would run the
    /// window's full height and square off the corners the fill has.
    private var selection: some View {
        RoundedRectangle(cornerRadius: Self.selectionRadius)
            .fill(Theme.selectedRowBackground.color)
            .overlay(alignment: .leading) {
                Rectangle()
                    .fill(Theme.accent.color)
                    .frame(width: Self.accentBarWidth)
            }
            .clipShape(RoundedRectangle(cornerRadius: Self.selectionRadius))
    }

    /// What a screen reader announces for the row.
    ///
    /// The date is deliberately left out. It is relative for anything active
    /// today, so a label carrying it would say something different one minute
    /// later and could not be pinned by a test.
    private var label: String {
        let pinned = conversation.isPinned ? ", pinned" : ""
        return "\(title), \(eventCount)\(pinned)"
    }

    /// Shared with the filter, so a row can always be found by the words it
    /// shows. Two placeholders that drifted apart would make untitled
    /// conversations visible but unsearchable.
    private var title: String {
        ConversationFilter.displayTitle(of: conversation)
    }

    /// Pluralized by hand, and `verbatim` so neither this nor the title goes
    /// through a localization lookup.
    ///
    /// `^[\(count) event](inflect: true)` reads better but resolves grammatical
    /// agreement at runtime, once per row, every time the list realizes one.
    private var eventCount: String {
        conversation.eventsCount == 1 ? "1 event" : "\(conversation.eventsCount) events"
    }
}
