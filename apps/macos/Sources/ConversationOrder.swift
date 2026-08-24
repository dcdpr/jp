/// Puts the conversation list in the order the sidebar shows it.
///
/// Pure, and separate from the library's own ordering on purpose: the library
/// reports conversations most recently active first, which is a fact about the
/// data, and where a pinned conversation belongs in a list is a decision about
/// the interface.
enum ConversationOrder {
    /// `conversations` with the pinned ones first.
    ///
    /// A stable partition: inside each group the given order is kept, so pinning
    /// a conversation lifts it to the top and moves nothing else. Pinned
    /// conversations stay most recently active first among themselves.
    static func pinnedFirst(_ conversations: [ConversationSummary]) -> [ConversationSummary] {
        let pinned = conversations.filter(\.isPinned)

        // The common case, and worth the check: this runs on every keystroke in
        // the filter box, over every conversation the workspace holds.
        guard !pinned.isEmpty else { return conversations }

        return pinned + conversations.filter { !$0.isPinned }
    }

    /// The rows that draw no line under them, given what is selected.
    ///
    /// The selected row and the one above it, so the selection's rounded fill is
    /// not cut across by a separator at either end of it. Empty when nothing is
    /// selected, and when the selection is not in the list — which happens while a
    /// filter is hiding the selected conversation.
    static func rowsWithoutSeparator(
        in conversations: [ConversationSummary],
        selecting selection: ConversationSummary.ID?
    ) -> Set<ConversationSummary.ID> {
        guard
            let selection,
            let index = conversations.firstIndex(where: { $0.id == selection })
        else { return [] }

        guard index > conversations.startIndex else { return [selection] }

        return [selection, conversations[index - 1].id]
    }
}
