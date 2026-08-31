import AppKit
import Foundation

/// Where the recent-workspace list is kept.
///
/// Storing the list is all this covers. Canonicalizing paths and dropping
/// directories that have gone away are policy ``RecentWorkspaces`` applies above
/// it, so every implementation agrees on them.
@MainActor
protocol RecentsStore {
    /// The recorded workspaces, most recent first.
    func urls() -> [URL]

    /// Record a workspace as opened, moving it to the front.
    func note(_ url: URL)

    /// Forget every recorded workspace.
    func clear()
}

/// The recent-workspace list as AppKit keeps it.
///
/// `NSDocumentController`'s list persists across launches and is shared with the
/// system, which is what puts the app's workspaces in its Dock menu. It is keyed
/// by bundle identifier, so every process running this app reads and writes one
/// list — the test bundle included, since the tests run hosted by the app.
struct DocumentControllerRecents: RecentsStore {
    func urls() -> [URL] {
        NSDocumentController.shared.recentDocumentURLs
    }

    func note(_ url: URL) {
        NSDocumentController.shared.noteNewRecentDocumentURL(url)
    }

    func clear() {
        NSDocumentController.shared.clearRecentDocuments(nil)
    }
}

/// The recent-workspace list kept as JSON at a path of the caller's choosing.
///
/// Paths are stored as an array of strings, most recent first, so a harness can
/// read the list it drove the app into directly rather than through the
/// accessibility tree:
///
/// ```json
/// ["/Users/jean/Projects/jp", "/tmp/probe-ws"]
/// ```
///
/// Nothing is cached: every call reads the file. The list holds ten paths and a
/// harness may rewrite it between launches, so there is nothing here worth the
/// risk of serving a stale answer.
struct FileRecents: RecentsStore {
    /// The JSON file backing the list, created on the first ``note(_:)``.
    let path: URL

    /// How many paths the list keeps, matching what `NSDocumentController` stores
    /// by default.
    static let capacity = 10

    /// The recorded workspaces, most recent first.
    ///
    /// A file that is not there yet is an empty list rather than an error: that is
    /// the state before anything has been opened. A file that is there but
    /// unreadable is reported and also read as empty, because refusing to produce
    /// a list would cost the window its workspace.
    ///
    /// `isDirectory: false` keeps the round-trip verbatim. The plain
    /// `URL(fileURLWithPath:)` consults the filesystem and appends a slash to a
    /// path that names a directory, so a path would read back spelled differently
    /// from how it was written and ``note(_:)`` would stop recognizing it.
    func urls() -> [URL] {
        guard let data = try? Data(contentsOf: path) else {
            return []
        }

        do {
            let paths = try JSONDecoder().decode([String].self, from: data)
            return paths.map { URL(fileURLWithPath: $0, isDirectory: false) }
        } catch {
            report("could not read \(path.path(percentEncoded: false)): \(error)")
            return []
        }
    }

    func note(_ url: URL) {
        let noted = url.path(percentEncoded: false)
        var paths = urls().map { $0.path(percentEncoded: false) }

        // Dropping any earlier spelling of the same path before inserting is what
        // makes this a move-to-front rather than a second entry.
        paths.removeAll { $0 == noted }
        paths.insert(noted, at: 0)

        write(Array(paths.prefix(Self.capacity)))
    }

    func clear() {
        write([])
    }

    private func write(_ paths: [String]) {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted]

        do {
            try FileManager.default.createDirectory(
                at: path.deletingLastPathComponent(),
                withIntermediateDirectories: true
            )
            try encoder.encode(paths).write(to: path, options: .atomic)
        } catch {
            report("could not write \(path.path(percentEncoded: false)): \(error)")
        }
    }

    /// Note a failure on stderr.
    ///
    /// The list is a convenience, and a launch that cannot persist it should still
    /// open a window. Reporting rather than throwing keeps that true, and stderr
    /// is where a harness driving the app is already reading.
    private func report(_ message: String) {
        FileHandle.standardError.write(Data("recents: \(message)\n".utf8))
    }
}
