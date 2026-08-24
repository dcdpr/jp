import Foundation

/// Where a UI test writes what a reader needs and `xcodebuild` will not carry.
///
/// Two things end up here: screenshots of what was on screen when an assertion
/// failed, and the failure messages themselves. The messages need a home
/// because swift-testing prints an issue's text on a line of its own, under a
/// header naming only the kind of issue, and `xcodebuild` keeps the header and
/// drops the line — so ten failures arrive as ten identical `Issue recorded`
/// entries, which says how many things broke and nothing about what.
///
/// The directory is the runner's container, not the checkout. Xcode wraps a UI
/// test bundle in a generated, sandboxed runner app, so a write anywhere in the
/// project fails with `Operation not permitted` however the path is spelled.
/// `swift_test_ui` copies out of here and into `tmp/uitests/`.
enum Diagnostics {
    /// The directory both screenshots and messages are written to.
    static let directory = FileManager.default.temporaryDirectory
        .appendingPathComponent("jp-uitests")

    /// Where the messages are written.
    static let file = directory.appendingPathComponent("failures.txt")

    /// Where the process ids of the apps this run launched are written.
    ///
    /// A run stopped part-way is stopped from outside, by killing
    /// `xcodebuild`. That does not reach the app: it is `testmanagerd` that
    /// launched it, so it survives and stays on screen. These are how the tool
    /// that stopped the run finds it, exactly, without matching on a name the
    /// developer's own copy of JP also has.
    static let processes = directory.appendingPathComponent("app.pids")

    /// Note that an app was launched, so a stopped run can still close it.
    static func recordAppProcess(_ pid: String) {
        append(pid, to: processes)
    }

    /// Append one line, creating the file if this is the first.
    ///
    /// Silent on failure. This runs while a test is already failing, and a
    /// second failure would bury the first.
    static func append(_ line: String) {
        append(line, to: file)
    }

    private static func append(_ line: String, to file: URL) {
        guard let data = (line + "\n").data(using: .utf8) else { return }

        try? FileManager.default.createDirectory(
            at: directory,
            withIntermediateDirectories: true
        )

        guard let handle = try? FileHandle(forWritingTo: file) else {
            try? data.write(to: file)
            return
        }

        defer { try? handle.close() }

        do {
            try handle.seekToEnd()
            try handle.write(contentsOf: data)
        } catch {
            // Nothing useful left to do: the test is already failing, and the
            // message is on its way to `Issue.record` regardless.
        }
    }
}
