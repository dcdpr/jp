import SwiftUI

/// What the history pane has to show.
///
/// One value rather than separate properties, for the same reason as
/// ``WorkspaceState``: a load result reaches the view in a single mutation.
enum TranscriptState: Equatable, Sendable {
    /// A conversation is being read.
    case loading

    /// A conversation's turns, oldest first, ready to draw, and which
    /// conversation they came from.
    ///
    /// The identifier travels with the turns rather than being read from the
    /// view, because the two disagree for as long as a newly selected
    /// conversation is still being read — the pane goes on showing the last one.
    case loaded(id: String, turns: [ConversationTurn])

    /// There is nothing to show, and why.
    case unavailable(title: String, detail: String)

    /// Whether there is a transcript on screen worth keeping while another
    /// loads.
    var hasContent: Bool {
        if case .loaded = self { true } else { false }
    }
}

/// The selected conversation, rendered as a scrolling transcript.
struct ConversationHistoryView: View {
    let model: WorkspaceModel
    let conversationID: ConversationSummary.ID?

    @State private var state: TranscriptState = .unavailable(
        title: "No Conversation Selected",
        detail: "Pick a conversation to read it."
    )

    var body: some View {
        Trace.measuring("ConversationHistoryView.body", target: Self.traceTarget) {
            content
        }
    }

    /// What the pane shows, timed by ``body``.
    private var content: some View {
        Group {
            switch state {
            case .loading:
                ProgressView()
                    .controlSize(.small)
                    .accessibilityLabel("Loading conversation")
                    .accessibilityIdentifier(AccessibilityID.Transcript.loadingState)

            case .loaded(let id, let turns):
                TranscriptTextView(conversationID: id, turns: turns)
                    // One text view per conversation, so switching builds a new
                    // one rather than moving a new transcript into the old one.
                    // Sharing it kept the scroll offset across a switch, because
                    // nothing told it the content underneath had been replaced.
                    //
                    // Keyed on the conversation *on screen*, not the one
                    // selected. Keyed on the selection it changed the moment a
                    // row was clicked, which built a second text view around the
                    // outgoing transcript and paid for the whole document again
                    // before the new one had even been read.
                    .id(id)

            case .unavailable(let title, let detail):
                ContentUnavailableView(
                    title,
                    systemImage: "bubble.left.and.text.bubble.right",
                    description: Text(detail)
                )
                .accessibilityIdentifier(AccessibilityID.Transcript.unavailableState)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.editorBackground.color)
        // Keyed on the *open* workspace, not the requested one. A window opened
        // straight onto a conversation renders while its workspace is still
        // opening, and keying on the request would fire that first read against
        // no session and never try again.
        .task(id: ReadKey(workspace: model.openWorkspace, conversation: conversationID)) {
            await load()
        }
    }

    /// What this pane's events are attributed to.
    private static let traceTarget = "JP.Transcript"

    /// The interval covering one conversation being picked and read.
    ///
    /// Named once because the read nested inside it reports it as its enclosing
    /// span, and two spellings would break that link.
    ///
    /// Covers the read alone. Building the text and laying it out happen later,
    /// while the view draws, and are timed there as `transcript.render`.
    private static let selectionSpan = "conversation.select"

    /// What a reload depends on.
    private struct ReadKey: Equatable {
        let workspace: String?
        let conversation: String?
    }

    private func load() async {
        guard let conversationID else {
            state = .unavailable(
                title: "No Conversation Selected",
                detail: "Pick a conversation to read it."
            )
            return
        }

        // Nothing to read until the workspace is open; the task runs again when
        // it is.
        guard model.openWorkspace != nil else { return }

        // The transcript already on screen stays there until the next one is
        // ready. Clearing first put an empty pane between the two, which reads as
        // a flash when the read takes a few milliseconds. Only an empty pane gets
        // a spinner, because there is nothing to keep.
        if !state.hasContent {
            state = .loading
        }

        let timing = Trace.interval(Self.selectionSpan, target: Self.traceTarget)
        let result = await model.events(for: conversationID, spans: [Self.selectionSpan])

        // Selecting another conversation cancels this task, but the read it
        // started still finishes, and its result must not replace the new one.
        guard !Task.isCancelled else {
            timing.end([("cancelled", true)])
            return
        }

        let next: TranscriptState =
            switch result {
            case .success(let turns) where turns.isEmpty:
                .unavailable(
                    title: "Empty Conversation",
                    detail: "This conversation has no messages yet."
                )
            case .success(let turns):
                .loaded(id: conversationID, turns: turns)
            case .failure(let error):
                .unavailable(title: "Could Not Read Conversation", detail: error.message)
            }

        // Animated at the assignment rather than through `.animation(value:)`,
        // which would compare the whole transcript — every message — on each
        // change to decide whether to animate.
        withAnimation(DebugState.animated(.easeInOut(duration: 0.12))) {
            state = next
        }

        timing.end()
    }
}
