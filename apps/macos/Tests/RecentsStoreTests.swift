import Foundation
import Testing

@testable import JP

/// ``FileRecents`` on its own, without the canonicalizing and pruning
/// ``RecentWorkspaces`` layers on top. Paths here need not exist on disk.
@MainActor
@Suite("FileRecents")
struct FileRecentsTests {
    /// A store at `recents.json` inside a directory of its own.
    private func makeStore(in root: URL) -> FileRecents {
        FileRecents(path: root.appendingPathComponent("recents.json"))
    }

    @Test("reads an absent file as an empty list")
    func readsAnAbsentFileAsEmpty() throws {
        let root = try makeTemporaryDirectory()
        defer { removeSandbox(root) }

        #expect(makeStore(in: root).urls().isEmpty)
    }

    @Test("reads back what it wrote, most recent first")
    func roundTripsInOrder() throws {
        let root = try makeTemporaryDirectory()
        defer { removeSandbox(root) }

        let store = makeStore(in: root)
        store.note(URL(fileURLWithPath: "/one"))
        store.note(URL(fileURLWithPath: "/two"))

        #expect(store.urls().map { $0.path(percentEncoded: false) } == ["/two", "/one"])
    }

    @Test("moves a repeated path to the front rather than duplicating it")
    func movesARepeatedPathToTheFront() throws {
        let root = try makeTemporaryDirectory()
        defer { removeSandbox(root) }

        let store = makeStore(in: root)
        store.note(URL(fileURLWithPath: "/one"))
        store.note(URL(fileURLWithPath: "/two"))
        store.note(URL(fileURLWithPath: "/one"))

        #expect(store.urls().map { $0.path(percentEncoded: false) } == ["/one", "/two"])
    }

    @Test("keeps only the most recent paths")
    func keepsOnlyTheMostRecentPaths() throws {
        let root = try makeTemporaryDirectory()
        defer { removeSandbox(root) }

        let store = makeStore(in: root)
        for index in 0...FileRecents.capacity {
            store.note(URL(fileURLWithPath: "/workspace-\(index)"))
        }

        let paths = store.urls().map { $0.path(percentEncoded: false) }
        #expect(paths.count == FileRecents.capacity)
        #expect(paths.first == "/workspace-\(FileRecents.capacity)")
        #expect(paths.last == "/workspace-1")
    }

    /// The file is a harness's to write, so it can arrive malformed. An empty list
    /// costs the menu its entries; refusing to produce one would cost the window
    /// its workspace.
    @Test("reads a malformed file as an empty list")
    func readsAMalformedFileAsEmpty() throws {
        let root = try makeTemporaryDirectory()
        defer { removeSandbox(root) }
        let store = makeStore(in: root)
        try "not json".write(to: store.path, atomically: true, encoding: .utf8)

        #expect(store.urls().isEmpty)
    }

    @Test("clears the file")
    func clearsTheFile() throws {
        let root = try makeTemporaryDirectory()
        defer { removeSandbox(root) }

        let store = makeStore(in: root)
        store.note(URL(fileURLWithPath: "/one"))
        store.clear()

        #expect(store.urls().isEmpty)
    }

    /// The tools write the file before the app has ever run, into a directory that
    /// may not exist yet.
    @Test("creates the directory it writes into")
    func createsTheDirectory() throws {
        let root = try makeTemporaryDirectory()
        defer { removeSandbox(root) }
        let store = FileRecents(path: root.appendingPathComponent("state/recents.json"))

        store.note(URL(fileURLWithPath: "/one"))

        #expect(store.urls().map { $0.path(percentEncoded: false) } == ["/one"])
    }
}
