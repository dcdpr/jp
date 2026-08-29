import SwiftUI

/// One conversation, in a window of its own.
///
/// Opened by double-clicking a conversation, and restored at launch from the
/// reference the system kept, which is why a reference carries its workspace
/// path: there may be no workspace window open to ask.
struct ConversationWindow: View {
    /// The scene identifier `openWindow` addresses this group by.
    static let sceneID = "conversation"

    let reference: ConversationRef?

    @State private var model = WorkspaceModel()

    var body: some View {
        Group {
            if let reference {
                ConversationHistoryView(model: model, conversationID: reference.conversationID)
                    .navigationTitle(reference.displayTitle)
                    // Reachable from whichever Space is on screen, for a driven
                    // build. See ``DebugSpaces``.
                    .background(DebugSpaces.joinEverySpace())
            } else {
                ContentUnavailableView(
                    "No Conversation",
                    systemImage: "bubble.left.and.text.bubble.right",
                    description: Text("This window has nothing to show.")
                )
            }
        }
        .task(id: reference) { await load() }
    }

    /// Open the reference's workspace, which this window does not share with the
    /// one the conversation came from.
    private func load() async {
        guard let reference, !reference.workspacePath.isEmpty else { return }
        await model.open(reference.workspacePath)
    }
}
