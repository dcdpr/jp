import Foundation

/// Narrows the conversation list to what a person typed.
///
/// Pure, so the matching rules can be pinned without a window: what counts as a
/// match is a product decision, and the place it is decided should not need a
/// running app to inspect.
enum ConversationFilter {
    /// The conversations whose title contains `query`.
    ///
    /// A blank query matches everything, so clearing the box restores the list
    /// rather than emptying it.
    ///
    /// Matching is on the title as the row displays it, including the placeholder
    /// an untitled conversation shows: filtering a list means filtering what is on
    /// screen, and a row a person can read but not search for is a surprise.
    /// Conversation IDs are deliberately not searched — they are timestamps, and
    /// matching them would let a query hit rows with no visible reason.
    ///
    /// Order is preserved, so the list stays most recently active first.
    static func matches(
        _ conversations: [ConversationSummary], query: String
    )
        -> [ConversationSummary]
    {
        let query = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { return conversations }

        return conversations.filter { displayTitle(of: $0).localizedStandardContains(query) }
    }

    /// The title a row shows for a conversation.
    ///
    /// Untitled conversations are common — a title is generated after the first
    /// turn — so the placeholder is part of what the list displays and part of
    /// what a query searches.
    static func displayTitle(of conversation: ConversationSummary) -> String {
        return conversation.title ?? "Untitled"
    }
}
