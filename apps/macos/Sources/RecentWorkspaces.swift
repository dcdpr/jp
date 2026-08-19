import Foundation
import Observation

/// The workspaces opened before, most recent first.
///
/// Reads and writes the list through a ``RecentsStore``, and owns the two rules
/// that apply whichever store is in use: paths are canonicalized on the way in,
/// and directories that have gone away are dropped on the way out.
///
/// The `File ▸ Open Recent` menu is built from ``urls`` explicitly. AppKit manages
/// that menu on its own only for a document-based app, which this is not.
@MainActor
@Observable
final class RecentWorkspaces {
    private(set) var urls: [URL] = []

    private let store: any RecentsStore

    init(store: any RecentsStore) {
        self.store = store
        urls = Self.pruned(store.urls())
    }

    /// A list backed by whichever store the app's environment selects.
    convenience init() {
        self.init(store: DebugState.defaultStore())
    }

    /// Record a workspace as opened, moving it to the front.
    ///
    /// The URL is canonicalized first. `NSDocumentController` resolves symlinks
    /// when it stores one, and macOS symlinks `/var` and `/tmp`, so noting a URL
    /// as given would put a path in the menu that never matches the one a window
    /// was opened with — and windows are keyed by path, so the same workspace
    /// would open twice.
    func note(_ url: URL) {
        store.note(url.canonicalized)
        urls = Self.pruned(store.urls())
    }

    /// Forget every recorded workspace.
    func clear() {
        store.clear()
        urls = Self.pruned(store.urls())
    }

    /// The recorded workspaces that still exist on disk, canonicalized.
    ///
    /// A directory can be deleted or unmounted between launches, and offering to
    /// open one that is gone only produces an error the user cannot act on.
    private static func pruned(_ urls: [URL]) -> [URL] {
        urls.map(\.canonicalized).filter { url in
            var isDirectory: ObjCBool = false
            let exists = FileManager.default.fileExists(
                atPath: url.path(percentEncoded: false),
                isDirectory: &isDirectory
            )
            return exists && isDirectory.boolValue
        }
    }
}

extension URL {
    /// The URL with symlinks resolved and any trailing slash dropped, so two
    /// spellings of one directory compare equal.
    ///
    /// `URL(fileURLWithPath:)` checks the filesystem and marks an existing
    /// directory as one, which puts a trailing slash into every path read back out.
    /// Windows are keyed by that path, so a list holding `/a/b/` while a window is
    /// keyed by `/a/b` lets one workspace open twice.
    ///
    /// `isDirectory: false` is what keeps the slash off, and is not a claim about
    /// what is at the path: it declares the spelling rather than letting the
    /// filesystem pick one, which is the whole point of a canonical form.
    var canonicalized: URL {
        let path = resolvingSymlinksInPath().path(percentEncoded: false)
        let trimmed =
            path.count > 1 && path.hasSuffix("/") ? String(path.dropLast()) : path

        return URL(fileURLWithPath: trimmed, isDirectory: false)
    }
}
