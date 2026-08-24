import SwiftUI

/// The box that narrows the conversation list.
///
/// Built rather than styled, because none of the stock text field styles gives a
/// glyph inside the field, and the bordered ones draw a focus ring the design
/// does not have.
///
/// Carries no outer padding, so a caller can place it against the window buttons
/// and give it the height it needs to line up with them.
struct SearchField: View {
    /// What has been typed.
    @Binding var text: String

    /// The corner radius of the field and of its border, which have to match or
    /// the stroke cuts across the fill.
    private static let radius: CGFloat = 6

    var body: some View {
        HStack(spacing: 5) {
            // Hidden from the accessibility tree: it says nothing the field's own
            // label does not, and SwiftUI otherwise publishes an SF Symbol as an
            // element identified by its symbol name — a name nothing here chose,
            // sitting in the tree beside the ones that were.
            Image(systemName: "magnifyingglass")
                .foregroundStyle(Theme.secondaryText.color)
                .accessibilityHidden(true)

            // The accessibility modifiers sit directly on the field, ahead of
            // the layout ones, so they cannot land on a wrapper `padding`
            // introduces.
            //
            // A collapsed sidebar takes the field out of the accessibility tree
            // entirely, along with the list. A driver that cannot find either
            // should check the sidebar is showing before concluding an
            // identifier is missing.
            // An empty title, with the placeholder drawn below instead: a
            // `TextField`'s own placeholder takes the system's grey and no
            // modifier reaches it, which leaves it several shades lighter than
            // every other piece of secondary text in the sidebar.
            TextField("", text: $text)
                .accessibilityLabel("Filter conversations")
                .accessibilityIdentifier(AccessibilityID.Sidebar.filter)
                .textFieldStyle(.plain)
                .foregroundStyle(Theme.bodyText.color)
                .background(alignment: .leading) {
                    if text.isEmpty {
                        // Never a click target, or it would swallow the click that
                        // is meant to put the caret in the field.
                        Text(verbatim: "Filter")
                            .foregroundStyle(Theme.secondaryText.color)
                            .allowsHitTesting(false)
                            // The field already carries this as its label, so
                            // publishing it again would put two elements in the
                            // tree saying the same thing.
                            .accessibilityHidden(true)
                    }
                }

            // Always there, whether or not there is anything to clear. A control
            // that comes and goes with what has been typed moves the text's right
            // edge as it appears, and the field is the one part of the sidebar
            // that should not shift while somebody is typing into it.
            Button {
                text = ""
            } label: {
                Image(systemName: "xmark.circle.fill")
                    .foregroundStyle(Theme.secondaryText.color)
            }
            .accessibilityLabel("Clear the filter")
            .accessibilityIdentifier(AccessibilityID.Sidebar.filterClear)
            .buttonStyle(.plain)
        }
        .font(.system(size: 13))
        .padding(.horizontal, 8)
        .padding(.vertical, 8)
        .background(
            RoundedRectangle(cornerRadius: Self.radius)
                .fill(Theme.searchFieldBackground.color)
                // The field and the sidebar are the same colour in both
                // appearances, so the border is the only thing that says where
                // the field is.
                .overlay(
                    RoundedRectangle(cornerRadius: Self.radius)
                        .strokeBorder(Theme.paneDivider.color, lineWidth: 1)
                )
        )
    }
}
