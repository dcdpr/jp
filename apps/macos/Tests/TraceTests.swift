import Foundation
import Testing

@testable import JP

/// One line exactly as the app writes it.
///
/// Pinned here and in `.config/jp/tools/src/debug_app/trace_tests.rs`, character
/// for character. Nothing else checks that the writer and the reader agree on
/// the format: if one of these two strings is edited alone, the other test is
/// what says so.
private let appLine = """
    {"timestamp":"2026-08-02T11:04:12.418293Z","level":"INFO","target":"JP.Transcript",\
    "fields":{"message":"transcript.render","duration_ms":84.219,"event_count":847,\
    "footprint_mb":412},"spans":[{"name":"conversation.select"}]}
    """

@Suite("Trace")
struct TraceTests {
    @Test("writes the line the tooling parses")
    func writesThePinnedLine() throws {
        let line = try #require(
            Trace.line(
                timestamp: "2026-08-02T11:04:12.418293Z",
                level: .info,
                target: "JP.Transcript",
                message: "transcript.render",
                fields: [
                    ("duration_ms", 84.219),
                    ("event_count", 847),
                    ("footprint_mb", 412),
                ],
                spans: ["conversation.select"]
            )
        )

        #expect(line == appLine)
    }

    /// The parser treats `spans` as optional, and most events are not nested
    /// inside anything.
    @Test("leaves the span stack out when there is none")
    func omitsAnEmptySpanStack() throws {
        let line = try #require(
            Trace.line(
                timestamp: "2026-08-02T11:04:10.000000Z",
                level: .info,
                target: "JP.Trace",
                message: "trace.origin",
                fields: [("timebase_numer", 125), ("timebase_denom", 3)],
                spans: []
            )
        )

        #expect(
            line == """
                {"timestamp":"2026-08-02T11:04:10.000000Z","level":"INFO","target":"JP.Trace",\
                "fields":{"message":"trace.origin","timebase_numer":125,"timebase_denom":3}}
                """
        )
    }

    /// RFC 3339, UTC, fractional seconds. A timestamp in local time or without
    /// the fraction still parses, and lands the event in the wrong place on a
    /// timeline drawn beside `jp`'s.
    @Test("formats timestamps as UTC to the microsecond")
    func formatsTimestamps() {
        #expect(
            Trace.timestamp(Date(timeIntervalSince1970: 1_785_668_652.418293))
                == "2026-08-02T11:04:12.418293Z"
        )
        #expect(
            Trace.timestamp(Date(timeIntervalSince1970: 0)) == "1970-01-01T00:00:00.000000Z")
    }

    @Test("reports the process footprint")
    func reportsTheFootprint() throws {
        let footprint = try #require(Trace.footprintMB())

        // A live process occupies something, and a footprint of hundreds of
        // gigabytes would mean the struct was read as the wrong shape.
        #expect(footprint > 0)
        #expect(footprint < 100_000)
    }

    @Test("converts mach ticks to milliseconds")
    func convertsMachTicks() {
        let timebase = MachTimebase.current
        // Exactly one second's worth of ticks, whatever this machine counts in.
        let ticks =
            UInt64(1_000_000_000) * UInt64(timebase.denominator)
            / UInt64(timebase.numerator)

        #expect(abs(Trace.milliseconds(ticks) - 1000) < 0.01)
    }
}

/// Nested in ``WorkspaceSuite`` because these read `JP_DEBUG_STATE_DIR`, which
/// the whole process shares.
extension WorkspaceSuite {
    @Suite("TraceWriter")
    struct TraceWriterTests {
        /// The shipping configuration, and the one thing this must never get
        /// wrong: an installed app writing a trace file into someone's disk
        /// would be a defect, not a feature.
        @Test("creates nothing without a state directory")
        func createsNothingWithoutADirectory() throws {
            let root = try makeTemporaryDirectory()
            defer { removeSandbox(root) }

            #expect(TraceWriter(directory: nil, fileName: Trace.fileName) == nil)
            #expect(try FileManager.default.contentsOfDirectory(atPath: root.path).isEmpty)
        }

        /// The process-wide sink, resolved from the environment the test host
        /// runs under. Serialized with the tests that set that variable, so it
        /// is unset here.
        @Test("records nothing when the app was launched as it ships")
        func recordsNothingWhenLaunchedNormally() {
            #expect(ProcessInfo.processInfo.environment[DebugState.variable] == nil)
            #expect(Trace.isRecording == false)
            #expect(Trace.url == nil)

            // Reaches every sink there is. Nothing to assert but that it neither
            // crashes nor has a file to write to.
            Trace.event("test.event")
            Trace.interval("test.interval").end()
        }

        @Test("appends one line per event")
        func appendsOneLinePerEvent() throws {
            let root = try makeTemporaryDirectory()
            defer { removeSandbox(root) }

            let writer = try #require(TraceWriter(directory: root, fileName: Trace.fileName))
            writer.append("first")
            writer.append("second")

            #expect(writer.url == root.appendingPathComponent("trace.jsonl"))
            #expect(try String(contentsOf: writer.url, encoding: .utf8) == "first\nsecond\n")
        }

        /// The directory a harness names does not exist yet when it launches the
        /// app into it.
        @Test("creates the directory it was pointed at")
        func createsTheDirectory() throws {
            let root = try makeTemporaryDirectory()
            defer { removeSandbox(root) }
            let state = root.appendingPathComponent("state")

            let writer = try #require(TraceWriter(directory: state, fileName: Trace.fileName))
            writer.append("line")

            #expect(try String(contentsOf: writer.url, encoding: .utf8) == "line\n")
        }

        /// A relaunch truncates the file, and a second writer on the same path
        /// must not overwrite what the first one wrote.
        @Test("appends to a file that already has lines in it")
        func appendsToAnExistingFile() throws {
            let root = try makeTemporaryDirectory()
            defer { removeSandbox(root) }

            let first = try #require(TraceWriter(directory: root, fileName: Trace.fileName))
            first.append("first")

            let second = try #require(TraceWriter(directory: root, fileName: Trace.fileName))
            second.append("second")

            #expect(try String(contentsOf: second.url, encoding: .utf8) == "first\nsecond\n")
        }
    }
}
