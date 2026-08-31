import Foundation
import Testing

@testable import JP

/// Nested in ``WorkspaceSuite`` because these write `JP_DEBUG_STATE_DIR`, which the
/// whole process shares.
extension WorkspaceSuite {
    @MainActor
    @Suite("DebugState")
    struct DebugStateTests {
        /// Run `body` with `JP_DEBUG_STATE_DIR` set to `directory`, and unset after.
        ///
        /// Unset rather than restored: the variable belongs to a harness driving the
        /// app, so no test run has one to put back.
        private func withStateDirectory(_ directory: URL?, _ body: () throws -> Void) throws {
            if let directory {
                setenv(DebugState.variable, directory.path(percentEncoded: false), 1)
            } else {
                unsetenv(DebugState.variable)
            }
            defer { unsetenv(DebugState.variable) }

            try body()
        }

        /// The shipping configuration. A regression here would point the app's
        /// recents at a file nothing reads, silently.
        @Test("uses the system list when the variable is unset")
        func usesTheSystemListWhenUnset() throws {
            try withStateDirectory(nil) {
                #expect(DebugState.directory == nil)
                #expect(DebugState.defaultStore() is DocumentControllerRecents)
            }
        }

        /// The isolation the whole driving setup rests on: with the variable set, the
        /// app must not read or write the list it shares with the system.
        @Test("uses a file inside the state directory when the variable is set")
        func usesAFileWhenSet() throws {
            let root = try makeTemporaryDirectory()
            defer { removeSandbox(root) }

            try withStateDirectory(root) {
                let store = DebugState.defaultStore()
                let file = try #require(store as? FileRecents)
                #expect(file.path == root.appendingPathComponent("recents.json"))
            }
        }

        @Test("ignores an empty variable")
        func ignoresAnEmptyVariable() throws {
            setenv(DebugState.variable, "", 1)
            defer { unsetenv(DebugState.variable) }

            #expect(DebugState.directory == nil)
            #expect(DebugState.defaultStore() is DocumentControllerRecents)
        }

        /// How a harness that launched the app through `open(1)` learns which
        /// process it got.
        @Test("records the process id")
        func recordsTheProcessID() throws {
            let root = try makeTemporaryDirectory()
            defer { removeSandbox(root) }

            try withStateDirectory(root) {
                DebugState.recordProcessID()

                let recorded = try String(
                    contentsOf: root.appendingPathComponent("pid"),
                    encoding: .utf8
                )
                #expect(recorded == "\(getpid())\n")
            }
        }

        /// A profiler subtracts this from every address it samples, and a recorder
        /// that attached to an already-running app has no other way to learn it: the
        /// kernel's image-load events only exist in a trace that was already
        /// recording when dyld mapped the image.
        ///
        /// The format is a contract, not just a number. `Session::reported_slide` on
        /// the Rust side parses it as an unsigned integer, so a sign or an `0x`
        /// prefix would parse as nothing there and silently fall back to recovering
        /// the slide from the trace — which is the failure this file exists to
        /// avoid.
        @Test("records the main image's ASLR slide")
        func recordsTheImageSlide() throws {
            let root = try makeTemporaryDirectory()
            defer { removeSandbox(root) }

            try withStateDirectory(root) {
                DebugState.recordProcessID()

                let recorded = try String(
                    contentsOf: root.appendingPathComponent("slide"),
                    encoding: .utf8
                )
                #expect(recorded == "\(_dyld_get_image_vmaddr_slide(0))\n")

                let text = recorded.trimmingCharacters(in: .whitespacesAndNewlines)
                #expect(UInt64(text) != nil)
            }
        }

        /// The tools name a directory that does not exist yet, then launch the app
        /// into it.
        @Test("creates the state directory to record into")
        func createsTheStateDirectory() throws {
            let root = try makeTemporaryDirectory()
            defer { removeSandbox(root) }
            let state = root.appendingPathComponent("state")

            try withStateDirectory(state) {
                DebugState.recordProcessID()

                #expect(
                    FileManager.default.fileExists(
                        atPath: state.appendingPathComponent("pid").path(percentEncoded: false)
                    )
                )
            }
        }
    }
}
