import Foundation

/// What the conversation list has to show.
///
/// One value rather than a set of separate properties, so a load result reaches
/// the view in a single mutation. Assigning several observed properties in a row
/// makes the list reload partway through its own update, which AppKit reports as
/// a reentrant `NSTableView` delegate call.
enum WorkspaceState: Equatable, Sendable {
    /// A workspace is being read.
    case loading

    /// The workspace's conversations, most recently active first.
    case loaded([ConversationSummary])

    /// There is nothing to list, and why.
    case unavailable(title: String, detail: String)
}

/// The open workspace and its conversation list.
///
/// Loads once per opened workspace. Turns written by a concurrent `jp query` are
/// invisible until the workspace is reopened.
@MainActor
@Observable
final class WorkspaceModel {
    /// What the conversation list has to show.
    private(set) var state: WorkspaceState = .unavailable(
        title: "No Workspace",
        detail: "Choose File ▸ Open Workspace to browse a workspace."
    )

    /// The workspace being read, once one has been opened.
    ///
    /// The workspace this model was asked to open.
    private(set) var path: String?

    /// The workspace that is actually open and ready to read.
    ///
    /// Distinct from ``path``, which is set the moment a workspace is *requested*.
    /// A view that keyed a read on `path` would fire while the workspace was
    /// still opening, find nothing, and never try again.
    private(set) var openWorkspace: String?

    /// The workspace, held open for the life of the window.
    private var session: WorkspaceSession?

    /// Open the workspace containing `path`, replacing whatever was open.
    ///
    /// `path` may be the workspace root or any directory inside it.
    func open(_ path: String) async {
        self.path = path
        openWorkspace = nil
        session = nil
        state = .loading

        let timing = Trace.interval(Self.openSpan, target: Self.traceTarget)
        let opened = await WorkspaceSession.open(path: path)

        // The window can close while a read is in flight. Its task is cancelled,
        // but the read itself is not, so the result still arrives here.
        guard !Task.isCancelled else {
            timing.end([("cancelled", true)])
            return
        }

        switch opened {
        case .failure(let error):
            state = .unavailable(title: "Could Not Open Workspace", detail: error.message)
            timing.end([("failed", true)])

        case .success(let session):
            self.session = session
            openWorkspace = path

            let conversations = await session.readConversations(spans: [Self.openSpan])

            guard !Task.isCancelled else {
                timing.end([("cancelled", true)])
                return
            }

            state = Self.state(for: conversations)
            timing.end([("conversation_count", .int((try? conversations.get())?.count ?? 0))])
        }
    }

    /// What this model's events are attributed to.
    private static let traceTarget = "JP.Workspace"

    /// The interval opening a workspace and listing it is recorded as.
    private static let openSpan = "workspace.open"

    /// Read one conversation's turns from the open workspace.
    ///
    /// Reuses the open workspace rather than opening another: opening scans every
    /// conversation directory in both storage roots, which is far too much work
    /// to repeat every time somebody clicks a row.
    /// `spans` names the intervals already open around this call, root first, so
    /// the read is traced beneath the work that asked for it.
    func events(
        for conversationID: ConversationSummary.ID,
        spans: [String] = []
    ) async -> Result<[ConversationTurn], WorkspaceError> {
        guard let session else {
            return .failure(WorkspaceError(message: "No workspace is open."))
        }

        return await session.readEvents(for: conversationID, spans: spans)
    }

    /// The state a finished read leaves the list in.
    private static func state(
        for result: Result<[ConversationSummary], WorkspaceError>
    ) -> WorkspaceState {
        switch result {
        case .success(let conversations) where conversations.isEmpty:
            .unavailable(
                title: "No Conversations",
                detail: "This workspace has no conversations yet."
            )
        // Already ordered most recently active first by the library, which is
        // where that decision belongs: ordering timestamps needs them parsed, and
        // every caller re-deriving it is how two views of one workspace end up
        // disagreeing.
        case .success(let conversations):
            .loaded(conversations)
        case .failure(let error):
            .unavailable(title: "Could Not Read Workspace", detail: error.message)
        }
    }
}
