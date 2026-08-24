import CoreTransferable
import Foundation

/// A conversation, identified well enough to reopen from anywhere.
///
/// Carries the workspace path as well as the ID so a value copied, dragged, or
/// restored into a new window can be read without a window already having that
/// workspace open.
struct ConversationRef: Codable, Hashable, Sendable {
    let workspacePath: String
    let conversationID: String

    /// The title to show for the conversation, when one is known.
    ///
    /// Cosmetic, and absent on a value restored from disk, so nothing depends on
    /// it being present.
    var title: String?

    /// A window title that says something even when the title is unknown.
    var displayTitle: String {
        title ?? "Conversation \(conversationID)"
    }
}

extension ConversationRef: Transferable {
    /// How the conversation crosses a drag or a copy.
    ///
    /// Text, deliberately: a `jp://` URI is the form JP itself uses to reference
    /// a conversation, so a paste into a terminal, an editor, or a query is
    /// useful rather than opaque. A private binary type would only be readable by
    /// this app, which has nowhere to drop one yet.
    static var transferRepresentation: some TransferRepresentation {
        ProxyRepresentation(exporting: \.uri)
    }

    /// The conversation as a `jp://` URI.
    var uri: String {
        "jp://\(conversationID)"
    }
}
