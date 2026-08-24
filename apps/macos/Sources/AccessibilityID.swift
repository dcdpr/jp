/// Stable names for the elements an external accessibility driver has to find.
///
/// Every identifier is derived from identity, never from display text: renaming
/// a conversation, rewording an empty state, or localizing the app leaves all of
/// them unchanged. None of these are read out to a person.
///
/// The state identifiers are what lets a driver wait on a predicate instead of
/// sleeping — `sidebar.state.loading` disappearing and ``Sidebar/list``
/// appearing is the load completing.
enum AccessibilityID {
    /// The conversation list, and the two things that stand in for it.
    enum Sidebar {
        /// The conversation list.
        ///
        /// Exists only once the workspace has been read, so this is also the
        /// "sidebar loaded" predicate. There is no separate
        /// `sidebar.state.loaded`: the loaded sidebar is a single element, and a
        /// view carries one identifier.
        static let list = "sidebar.list"

        /// The box that narrows the list to matching conversations.
        static let filter = "sidebar.filter"

        /// The button that empties the filter box.
        ///
        /// Always present, whether or not the box holds anything, so it is not a
        /// predicate for the filter being in use.
        static let filterClear = "sidebar.filter.clear"

        /// The spinner shown while the workspace is being read.
        static let loadingState = "sidebar.state.loading"

        /// The message shown when a filter matches none of the conversations.
        ///
        /// Distinct from ``unavailableState``: the workspace was read and does
        /// hold conversations, so this says the query is wrong rather than that
        /// there is nothing to show.
        static let noMatchesState = "sidebar.state.nomatches"

        /// The message shown when there is no list to show, and why.
        static let unavailableState = "sidebar.state.unavailable"

        /// One row, named by the conversation it shows.
        static func row(_ conversation: ConversationSummary.ID) -> String {
            "sidebar.row.\(conversation)"
        }
    }

    /// The strip between the two panes that resizes the sidebar.
    ///
    /// Named because it is the one thing in the window a driver can only reach by
    /// dragging: the sidebar's width is not settable through the accessibility
    /// tree. Wider than the line it draws, so a pointer can hit it.
    static let paneDivider = "window.divider"

    /// The transcript pane and its contents.
    enum Transcript {
        /// The scrolling transcript.
        ///
        /// Exists only once a conversation has been read, so this is also the
        /// "transcript loaded" predicate, for the same reason as
        /// ``Sidebar/list``.
        static let scroll = "transcript.scroll"

        /// The spinner shown while a conversation is being read.
        static let loadingState = "transcript.state.loading"

        /// The message shown when there is no transcript, covering both no
        /// selection and a conversation that could not be read.
        static let unavailableState = "transcript.state.unavailable"

        /// The text the transcript is drawn as.
        ///
        /// The whole conversation is one text view, so there is no element per
        /// message to name. A driver addresses the transcript by this and reads
        /// its value, which is every message it is showing.
        static let text = "transcript.text"
    }
}
