import Foundation

/// A workspace held open, off the main actor.
///
/// Opening a workspace scans every conversation directory in both storage roots,
/// so it happens once per workspace rather than once per read. Reads are
/// serialized by the actor, which also keeps the reader — a noncopyable value
/// that cannot cross an isolation boundary — in one place.
actor WorkspaceSession {
    private let reader: WorkspaceReader

    /// Open the workspace containing `path`.
    ///
    /// `path` may be the workspace root or any directory inside it.
    init(path: String) throws(WorkspaceError) {
        reader = try WorkspaceReader(path: path)
    }

    /// Every conversation in the workspace.
    ///
    /// `spans` names the intervals already open around this call, root first, so
    /// the read is traced beneath the work that asked for it.
    func conversations(spans: [String] = []) throws(WorkspaceError) -> [ConversationSummary] {
        try reader.conversations(spans: spans)
    }

    /// One conversation's events, oldest first.
    ///
    /// `spans` names the intervals already open around this call, root first.
    func events(
        for conversationID: String,
        spans: [String] = []
    ) throws(WorkspaceError) -> [ConversationTurn] {
        try reader.events(for: conversationID, spans: spans)
    }
}

extension WorkspaceSession {
    /// Open the workspace at `path`, off the main actor.
    static func open(path: String) async -> Result<WorkspaceSession, WorkspaceError> {
        let opened = Task.detached { () -> Result<WorkspaceSession, WorkspaceError> in
            do throws(WorkspaceError) {
                return .success(try WorkspaceSession(path: path))
            } catch {
                return .failure(error)
            }
        }

        return await opened.value
    }

    /// Read every conversation, returning the failure rather than throwing so a
    /// caller can put it on screen.
    func readConversations(
        spans: [String] = []
    ) async -> Result<[ConversationSummary], WorkspaceError> {
        do throws(WorkspaceError) {
            return .success(try conversations(spans: spans))
        } catch {
            return .failure(error)
        }
    }

    /// Read one conversation's events, returning the failure rather than
    /// throwing.
    func readEvents(
        for conversationID: String,
        spans: [String] = []
    ) async -> Result<[ConversationTurn], WorkspaceError> {
        do throws(WorkspaceError) {
            return .success(try events(for: conversationID, spans: spans))
        } catch {
            return .failure(error)
        }
    }
}
