import Foundation
import Testing

@testable import JP

/// A timings payload, character for character.
///
/// Pinned here and in `crates/jp_ffi/src/timing_tests.rs`, which asserts the
/// library produces this exact string. Nothing else checks that the two sides
/// agree on the shape: if one of these two literals is edited alone, the other
/// test is what says so.
private let timingsJSON = """
    [{"name":"storage.read","duration_ms":1.234},\
    {"name":"deserialize","duration_ms":84.219},\
    {"name":"serialize","duration_ms":3.0}]
    """

/// What the library reports about its own work, turned into trace events.
///
/// Decoding is pure, so these run outside ``WorkspaceSuite`` and in parallel
/// with it.
@Suite("LibraryTimings")
struct LibraryTimingsTests {
    /// The nesting is the point. An event recorded with an empty span stack
    /// still names `deserialize` and still carries a duration, and says nothing
    /// about which piece of app work paid for it — so the whole span stack is
    /// compared, not just the message.
    ///
    /// `3.0` comes back as `3`: `JSONEncoder` drops a trailing zero, and the
    /// trace parser reads either as a number.
    @Test("nests the library's spans under the app work that asked for them")
    func nestsUnderTheEnclosingSpans() {
        let lines = WorkspaceReader.timingLines(
            Data(timingsJSON.utf8),
            under: ["conversation.select", WorkspaceReader.eventsSpan],
            at: "2026-08-02T11:04:12.418293Z"
        )

        #expect(
            lines == [
                """
                {"timestamp":"2026-08-02T11:04:12.418293Z","level":"INFO","target":"JP.FFI",\
                "fields":{"message":"storage.read","duration_ms":1.234},\
                "spans":[{"name":"conversation.select"},{"name":"workspace.events"}]}
                """,
                """
                {"timestamp":"2026-08-02T11:04:12.418293Z","level":"INFO","target":"JP.FFI",\
                "fields":{"message":"deserialize","duration_ms":84.219},\
                "spans":[{"name":"conversation.select"},{"name":"workspace.events"}]}
                """,
                """
                {"timestamp":"2026-08-02T11:04:12.418293Z","level":"INFO","target":"JP.FFI",\
                "fields":{"message":"serialize","duration_ms":3},\
                "spans":[{"name":"conversation.select"},{"name":"workspace.events"}]}
                """,
            ]
        )
    }

    /// A field added on the Rust side must not stop an app built before it from
    /// reading the rest.
    @Test("ignores a key it does not know")
    func ignoresUnknownKeys() {
        let lines = WorkspaceReader.timingLines(
            Data(#"[{"name":"sort","duration_ms":0.5,"value_count":12}]"#.utf8),
            under: ["workspace.open", WorkspaceReader.conversationsSpan],
            at: "2026-08-02T11:04:12.418293Z"
        )

        #expect(
            lines == [
                """
                {"timestamp":"2026-08-02T11:04:12.418293Z","level":"INFO","target":"JP.FFI",\
                "fields":{"message":"sort","duration_ms":0.5},\
                "spans":[{"name":"workspace.open"},{"name":"workspace.conversations"}]}
                """
            ]
        )
    }

    /// What a call that failed before doing any of the work it measures reports.
    @Test("writes nothing for a call that measured nothing")
    func writesNothingForAnEmptyPayload() {
        #expect(
            WorkspaceReader.timingLines(
                Data("[]".utf8), under: ["conversation.select"],
                at: "2026-08-02T11:04:12.418293Z"
            ).isEmpty
        )
    }

    /// Instrumentation nobody can read is not a reason to fail the read it was
    /// measuring, so a payload that will not decode is dropped.
    @Test("writes nothing for a payload it cannot decode")
    func writesNothingForAMalformedPayload() {
        #expect(
            WorkspaceReader.timingLines(
                Data(#"{"name":"sort"}"#.utf8), under: ["conversation.select"],
                at: "2026-08-02T11:04:12.418293Z"
            ).isEmpty
        )
    }
}

/// End-to-end tests across the FFI boundary: they link the Rust static library
/// and call it, so a failure here means the seam is broken rather than the Swift
/// being wrong.
///
/// Nested in `WorkspaceSuite` because each test points `JP_USER_DATA_DIR` at its
/// own directory, and that variable belongs to the whole process.
extension WorkspaceSuite {
    @Suite("WorkspaceReader")
    struct WorkspaceReaderTests {

        /// The phase 2 goal in one assertion: the Rust library links, runs, and its
        /// output decodes into Swift values.
        @Test("opens a workspace and reads an empty conversation list")
        func opensAnEmptyWorkspace() throws {
            let sandbox = try makeSandbox()
            defer { removeSandbox(sandbox) }
            let path = try makeWorkspace(in: sandbox)

            let reader = try WorkspaceReader(path: path)
            let conversations = try reader.conversations()

            #expect(conversations.isEmpty)
        }

        /// Any directory inside the workspace opens the workspace, so the app can
        /// hand over whatever directory the user picked.
        @Test("opens a directory inside the workspace")
        func opensANestedDirectory() throws {
            let sandbox = try makeSandbox()
            defer { removeSandbox(sandbox) }
            let workspace = try makeWorkspace(in: sandbox)
            let nested = URL(fileURLWithPath: workspace)
                .appendingPathComponent("src/nested")
            try FileManager.default.createDirectory(
                at: nested, withIntermediateDirectories: true)

            let reader = try WorkspaceReader(path: nested.path)
            let conversations = try reader.conversations()

            #expect(conversations.isEmpty)
        }

        /// The library's failure message reaches Swift through the thread-local error
        /// slot, rather than being lost behind a null return.
        ///
        /// Assumes no workspace exists above the temporary directory. On a machine
        /// where one does, the open succeeds and this fails loudly instead of passing
        /// for the wrong reason.
        @Test("reports a directory that is not a workspace")
        func reportsANonWorkspace() throws {
            let sandbox = try makeSandbox()
            defer { removeSandbox(sandbox) }
            let path = try makeBareDirectory(in: sandbox, named: "not-a-workspace")

            do throws(WorkspaceError) {
                let reader = try WorkspaceReader(path: path)
                _ = try reader.conversations()
                Issue.record("expected opening a bare directory to fail")
            } catch {
                #expect(error.message == "No workspace found at or above: \(path)")
            }
        }

        /// A conversation ID that is not a decisecond timestamp is rejected by the
        /// library, not by Swift, so this proves the error crosses the boundary.
        @Test("reports an unparsable conversation ID")
        func reportsAnUnparsableConversationID() throws {
            let sandbox = try makeSandbox()
            defer { removeSandbox(sandbox) }
            let path = try makeWorkspace(in: sandbox)

            do throws(WorkspaceError) {
                let reader = try WorkspaceReader(path: path)
                _ = try reader.events(for: "not-an-id")
                Issue.record("expected an unparsable conversation ID to fail")
            } catch {
                #expect(error.message.hasPrefix("invalid conversation ID:"))
            }
        }

        @Test("reports a conversation that is not in the workspace")
        func reportsAMissingConversation() throws {
            let sandbox = try makeSandbox()
            defer { removeSandbox(sandbox) }
            let path = try makeWorkspace(in: sandbox)

            do throws(WorkspaceError) {
                let reader = try WorkspaceReader(path: path)
                _ = try reader.events(for: "17251488000")
                Issue.record("expected a missing conversation to fail")
            } catch {
                #expect(error.message.hasPrefix("conversation not found:"))
            }
        }

        /// The session the app reads through returns the failure rather than
        /// trapping, so a bad path shows up in the UI.
        @Test("surfaces a failure through the session")
        func sessionSurfacesFailures() async throws {
            let sandbox = try makeSandbox()
            defer { removeSandbox(sandbox) }
            let path = try makeBareDirectory(in: sandbox, named: "also-not-a-workspace")

            switch await WorkspaceSession.open(path: path) {
            case .success:
                Issue.record("expected opening a bare directory to fail")
            case .failure(let error):
                #expect(error.message.hasPrefix("No workspace found"))
            }
        }

        @Test("reads a workspace through the session")
        func sessionReadsAWorkspace() async throws {
            let sandbox = try makeSandbox()
            defer { removeSandbox(sandbox) }
            let path = try makeWorkspace(in: sandbox)

            guard case .success(let session) = await WorkspaceSession.open(path: path) else {
                Issue.record("expected the workspace to open")
                return
            }

            switch await session.readConversations() {
            case .success(let conversations):
                #expect(conversations.isEmpty)
            case .failure(let error):
                Issue.record("expected a successful read, got: \(error.message)")
            }
        }

        /// The whole point of holding a session open: many reads, one open. A
        /// second read must not need the workspace reopened.
        @Test("reads repeatedly from one open workspace")
        func sessionReadsRepeatedly() async throws {
            let sandbox = try makeSandbox()
            defer { removeSandbox(sandbox) }
            let path = try makeWorkspace(in: sandbox)

            guard case .success(let session) = await WorkspaceSession.open(path: path) else {
                Issue.record("expected the workspace to open")
                return
            }

            for _ in 0..<3 {
                switch await session.readConversations() {
                case .success(let conversations):
                    #expect(conversations.isEmpty)
                case .failure(let error):
                    Issue.record("expected a successful read, got: \(error.message)")
                }
            }
        }
    }
}
