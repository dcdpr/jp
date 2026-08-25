import Foundation
import Testing

@testable import JP

/// Outside ``WorkspaceSuite``, because each test owns the file its list is kept in
/// and so touches no state shared with the rest of the process. Backing these by
/// `NSDocumentController` would mean every run clearing the developer's own
/// `File ▸ Open Recent`.
@MainActor
@Suite("RecentWorkspaces")
struct RecentWorkspacesTests {
    /// A list backed by a file inside `root`.
    private func makeRecents(in root: URL) -> RecentWorkspaces {
        RecentWorkspaces(store: FileRecents(path: root.appendingPathComponent("recents.json")))
    }

    @Test("starts empty")
    func startsEmpty() throws {
        let root = try makeTemporaryDirectory()
        defer { removeSandbox(root) }

        #expect(makeRecents(in: root).urls.isEmpty)
    }

    @Test("records an opened workspace")
    func recordsAnOpenedWorkspace() throws {
        let root = try makeTemporaryDirectory()
        defer { removeSandbox(root) }
        let workspace = URL(fileURLWithPath: try makeWorkspace(in: root)).canonicalized

        let recents = makeRecents(in: root)
        recents.note(workspace)

        #expect(recents.urls == [workspace])
    }

    /// The temporary directory lives under a symlink, so a path recorded as given
    /// would never match the canonical one a window is keyed by.
    @Test("records a workspace under its canonical path")
    func canonicalizesTheRecordedPath() throws {
        let root = try makeTemporaryDirectory()
        defer { removeSandbox(root) }
        let workspace = URL(fileURLWithPath: try makeWorkspace(in: root))

        let recents = makeRecents(in: root)
        recents.note(workspace)

        #expect(recents.urls == [workspace.canonicalized])
    }

    /// A path read back out of the list is spelled the way a window is keyed by it.
    /// `URL(fileURLWithPath:)` marks an existing directory as one, so without
    /// normalizing, every entry carries a trailing slash the window keys do not.
    @Test("records a workspace without a trailing slash")
    func stripsATrailingSlash() throws {
        let root = try makeTemporaryDirectory()
        defer { removeSandbox(root) }
        let workspace = URL(fileURLWithPath: try makeWorkspace(in: root))

        let recents = makeRecents(in: root)
        recents.note(workspace)

        let recorded = try #require(recents.urls.first)
        #expect(!recorded.path(percentEncoded: false).hasSuffix("/"))
    }

    /// Reopening moves a workspace back to the front, which is what makes the
    /// menu ordering useful.
    @Test("puts the most recently opened workspace first")
    func putsTheMostRecentFirst() throws {
        let root = try makeTemporaryDirectory()
        defer { removeSandbox(root) }
        let first = URL(fileURLWithPath: try makeWorkspace(in: root, named: "first"))
            .canonicalized
        let second = URL(fileURLWithPath: try makeWorkspace(in: root, named: "second"))
            .canonicalized

        let recents = makeRecents(in: root)
        recents.note(first)
        recents.note(second)
        recents.note(first)

        #expect(recents.urls == [first, second])
    }

    /// A workspace can be deleted between launches, and offering to open one that
    /// is gone produces an error the user cannot act on.
    @Test("drops a workspace that no longer exists")
    func dropsAMissingWorkspace() throws {
        let root = try makeTemporaryDirectory()
        defer { removeSandbox(root) }
        let kept = URL(fileURLWithPath: try makeWorkspace(in: root, named: "kept"))
            .canonicalized
        let removed = URL(fileURLWithPath: try makeWorkspace(in: root, named: "removed"))
            .canonicalized

        let recents = makeRecents(in: root)
        recents.note(kept)
        recents.note(removed)
        #expect(recents.urls == [removed, kept])

        try FileManager.default.removeItem(at: removed)

        // A fresh instance reads the stored list, as a relaunch would.
        #expect(makeRecents(in: root).urls == [kept])
    }

    @Test("clears the list")
    func clearsTheList() throws {
        let root = try makeTemporaryDirectory()
        defer { removeSandbox(root) }
        let workspace = URL(fileURLWithPath: try makeWorkspace(in: root)).canonicalized

        let recents = makeRecents(in: root)
        recents.note(workspace)
        recents.clear()

        #expect(recents.urls.isEmpty)
        #expect(makeRecents(in: root).urls.isEmpty)
    }
}
