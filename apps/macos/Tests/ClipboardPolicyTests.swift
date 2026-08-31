import Foundation
import Testing

/// The UI suite must never touch the *system* pasteboard.
///
/// There is one of those and it belongs to whoever is at the keyboard. A test
/// that triggers a copy into it destroys what they had, and saving and
/// restoring around the test is not a fix: a pasteboard item can be a promise
/// its owner fulfils lazily, so a restore puts back a degraded copy and an
/// early exit puts back nothing at all.
///
/// A *named* pasteboard has none of that problem, so the UI tests use one: a
/// debug build reads `JP_DEBUG_PASTEBOARD` and copies there instead (see
/// ``DebugState/pasteboard``), and `WorkspaceFixture.copiedText()` reads it
/// back. Copy Link is covered end to end without a clipboard being lost.
///
/// What this forbids is therefore narrow and exact: the spellings that mean
/// "the one everybody shares". It is a source scan rather than a rule in a
/// document because a rule in a document is not enforced by anything.
@Suite("ClipboardPolicy")
struct ClipboardPolicyTests {
    /// The spellings that reach the system pasteboard.
    ///
    /// `NSPasteboard(name: .general)` is the same object as
    /// `NSPasteboard.general`, so naming it counts too.
    static let forbidden = [
        "NSPasteboard.general",
        "UIPasteboard.general",
        "Name.general",
        "name: .general",
    ]

    @Test("no UI test reaches for the system pasteboard")
    func uiTestsDoNotTouchThePasteboard() throws {
        let sources = try Self.uiTestSources()

        // A scan over nothing passes for the wrong reason, and would keep
        // passing if the directory were renamed.
        #expect(sources.count >= 3, "expected to find the UI test sources to scan")

        for source in sources {
            let text = try String(contentsOf: source, encoding: .utf8)
            for symbol in Self.forbidden where text.contains(symbol) {
                Issue.record(
                    """
                    \(source.lastPathComponent) reaches the system pasteboard through \
                    `\(symbol)`. Copy through the fixture's own pasteboard instead: the app \
                    writes to the one `JP_DEBUG_PASTEBOARD` names, and \
                    `WorkspaceFixture.copiedText()` reads it back.
                    """
                )
            }
        }
    }

    /// Every Swift file in `apps/macos/UITests`.
    ///
    /// Located from this file's compile-time path. The app is not sandboxed and
    /// these tests are hosted by it, so the checkout is readable from here.
    static func uiTestSources() throws -> [URL] {
        // .../apps/macos/Tests/ClipboardPolicyTests.swift
        let directory = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("UITests")

        return try FileManager.default
            .contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)
            .filter { $0.pathExtension == "swift" }
    }
}
