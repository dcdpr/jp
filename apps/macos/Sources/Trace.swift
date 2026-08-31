import Foundation
import os

/// What the app records about its own work.
///
/// Two sinks for the same intervals. `OSSignposter` always, so attaching
/// Instruments to any running instance shows them; and a line of JSON per event
/// to `<JP_DEBUG_STATE_DIR>/trace.jsonl` when a harness has pointed the app at a
/// directory, which is what the `debug_app_*` tools read back.
///
/// The file is its own channel rather than stdout or stderr, because those two
/// are reported as deltas on every snapshot: a trace stream on either would bury
/// what AppKit had to say under the app's own instrumentation.
///
/// With `JP_DEBUG_STATE_DIR` unset nothing is opened and no file is created, and
/// the only cost left is a signpost and a timestamp per interval.
enum Trace {
    /// The trace file, inside the debug state directory.
    static let fileName = "trace.jsonl"

    /// What an event is attributed to when the caller names nothing more
    /// specific.
    static let defaultTarget = "JP"

    /// Where the JSON goes, or `nil` when the app was launched as it ships.
    ///
    /// Resolved once. A harness sets the variable before launch and never
    /// changes it, and re-reading the environment per event would cost more than
    /// writing the line.
    private static let sink = TraceWriter(directory: DebugState.directory, fileName: fileName)

    /// The signpost stream Instruments shows.
    ///
    /// Named for the app rather than for the slot a driven copy runs under, so
    /// every instance appears under one subsystem.
    static let signposter = OSSignposter(
        subsystem: "computer.jp.jean-pierre", category: "trace")

    /// The signpost every interval is filed under.
    ///
    /// `OSSignposter` takes a `StaticString`, which an interval's name is not, so
    /// the name travels in the signpost's message instead.
    static let signpostName: StaticString = "interval"

    /// Whether events are being written to a file.
    static var isRecording: Bool { sink != nil }

    /// Where the file is, once there is one.
    static var url: URL? { sink?.url }

    /// Record one event.
    static func event(
        _ message: String,
        target: String = defaultTarget,
        level: TraceLevel = .info,
        fields: TraceFields = [],
        spans: [String] = []
    ) {
        guard isRecording else { return }

        let line = line(
            timestamp: timestamp(Date()),
            level: level,
            target: target,
            message: message,
            fields: fields,
            spans: spans
        )

        guard let line else { return }
        write(line)
    }

    /// Append a line that has already been built.
    ///
    /// For a caller assembling its own lines, such as one turning durations
    /// reported from elsewhere into events. ``event(_:target:level:fields:spans:)``
    /// is the ordinary way in.
    static func write(_ line: String) {
        sink?.append(line)
    }

    /// Start timing a piece of work, to be ended through the returned token.
    ///
    /// `fields` are written when the interval ends, before the ones `end` is
    /// given, so an interval's own context reads ahead of its result.
    static func interval(
        _ name: String,
        target: String = defaultTarget,
        fields: TraceFields = [],
        spans: [String] = []
    ) -> TraceInterval {
        TraceInterval(
            name: name,
            target: target,
            fields: fields,
            spans: spans,
            started: mach_absolute_time(),
            signpost: signposter.beginInterval(
                signpostName, id: signposter.makeSignpostID(), "\(name, privacy: .public)")
        )
    }

    /// Run `work` as an interval named `name`, and return what it produced.
    static func measuring<T>(
        _ name: String,
        target: String = defaultTarget,
        fields: TraceFields = [],
        spans: [String] = [],
        _ work: () -> T
    ) -> T {
        let token = interval(name, target: target, fields: fields, spans: spans)
        let value = work()
        token.end()
        return value
    }

    /// Write the event an interval ends with.
    ///
    /// The footprint is sampled here rather than in ``TraceInterval/end(_:)`` so
    /// an app running without a state directory never makes the call.
    static func record(_ interval: TraceInterval, elapsed ticks: UInt64, extra: TraceFields) {
        guard isRecording else { return }

        var fields: TraceFields = [("duration_ms", .double(milliseconds(ticks)))]
        fields.append(contentsOf: interval.fields)
        fields.append(contentsOf: extra)
        if let footprint = footprintMB() {
            fields.append(("footprint_mb", .int(footprint)))
        }

        event(
            interval.name,
            target: interval.target,
            fields: fields,
            spans: interval.spans
        )
    }

    /// Record the pair that lines this timeline up with one measured on the mach
    /// clock.
    ///
    /// A trace taken in Instruments carries mach timestamps and no wall clock;
    /// this file carries wall clocks and no mach timestamps. One reading of both
    /// at the same instant is what lets the two be laid over each other.
    static func origin() {
        let now = Date()
        let ticks = mach_absolute_time()
        let timebase = MachTimebase.current

        event(
            "trace.origin",
            target: "JP.Trace",
            fields: [
                ("mach_absolute_time", .int(Int(clamping: ticks))),
                ("unix_time_ns", .int(Int(now.timeIntervalSince1970 * 1_000_000_000))),
                ("timebase_numer", .int(Int(timebase.numerator))),
                ("timebase_denom", .int(Int(timebase.denominator))),
            ]
        )
    }

    /// One event as the line that goes in the file, or `nil` if it cannot be
    /// encoded.
    static func line(
        timestamp: String,
        level: TraceLevel,
        target: String,
        message: String,
        fields: TraceFields,
        spans: [String]
    ) -> String? {
        var all: TraceFields = [("message", .string(message))]
        all.append(contentsOf: fields)

        return TraceLine(
            timestamp: timestamp,
            level: level,
            target: target,
            fields: all,
            spans: spans
        ).encoded()
    }

    /// `date` as RFC 3339 in UTC, to the microsecond.
    ///
    /// Formatted by hand because `ISO8601DateFormatter` stops at milliseconds,
    /// and because a formatter is a reference type that would have to be shared
    /// across every thread that ends an interval.
    static func timestamp(_ date: Date) -> String {
        let seconds = date.timeIntervalSince1970
        let whole = seconds.rounded(.down)
        var epoch = time_t(whole)
        var parts = tm()
        gmtime_r(&epoch, &parts)

        // Rounded, not truncated: a `Date` holds seconds as a `Double`, and the
        // microsecond a caller put in comes back a fraction of a microsecond
        // short of itself.
        let micros = min(Int(((seconds - whole) * 1_000_000).rounded()), 999_999)

        return String(
            format: "%04d-%02d-%02dT%02d:%02d:%02d.%06dZ",
            parts.tm_year + 1900,
            parts.tm_mon + 1,
            parts.tm_mday,
            parts.tm_hour,
            parts.tm_min,
            parts.tm_sec,
            micros
        )
    }

    /// A span of `mach_absolute_time()` ticks in milliseconds, to the
    /// microsecond.
    static func milliseconds(_ ticks: UInt64) -> Double {
        let nanoseconds = Double(MachTimebase.current.nanoseconds(ticks))
        return (nanoseconds / 1000).rounded() / 1000
    }

    /// What the process currently occupies, in MiB.
    ///
    /// `phys_footprint` is the number macOS itself judges a process by, and the
    /// call to read it costs microseconds. Which call site allocated the bytes is
    /// a different question, and needs a tool that costs several times the run.
    static func footprintMB() -> Int? {
        var info = task_vm_info_data_t()
        var count = mach_msg_type_number_t(
            MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<natural_t>.size)

        let result = withUnsafeMutablePointer(to: &info) {
            $0.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
                task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count)
            }
        }

        guard result == KERN_SUCCESS else { return nil }
        return Int(clamping: info.phys_footprint / (1024 * 1024))
    }
}

extension Trace {
    /// The launch, held from the app's earliest code until its first window.
    ///
    /// Main-actor state rather than locked state: both ends of this particular
    /// interval run on the main actor, and nothing else touches it.
    @MainActor private static var launch: TraceInterval?

    /// Start the launch interval, and record the clock origin.
    @MainActor
    static func beginLaunch() {
        origin()
        launch = interval("app.launch", target: "JP.App")
    }

    /// End the launch interval, if it is still open.
    ///
    /// Called by every window as it appears, and only the first one finds an
    /// interval to end.
    @MainActor
    static func endLaunch() {
        launch?.end()
        launch = nil
    }
}

/// A started interval, ended by whoever holds it.
struct TraceInterval {
    /// What the interval is called, written as the event's message.
    let name: String

    /// What the event is attributed to.
    let target: String

    /// Context written ahead of whatever `end` is given.
    let fields: TraceFields

    /// The enclosing interval names, root first.
    let spans: [String]

    /// When it started, on the mach clock.
    let started: UInt64

    /// The signpost half of the same interval.
    let signpost: OSSignpostIntervalState

    /// Close the interval, writing how long it took and what the process now
    /// occupies.
    func end(_ extra: TraceFields = []) {
        let elapsed = mach_absolute_time() &- started
        Trace.signposter.endInterval(Trace.signpostName, signpost)
        Trace.record(self, elapsed: elapsed, extra: extra)
    }
}

/// Severity, spelled as the trace format spells it.
enum TraceLevel: String, Sendable {
    case trace = "TRACE"
    case debug = "DEBUG"
    case info = "INFO"
    case warn = "WARN"
    case error = "ERROR"
}

/// What a trace field can hold.
enum TraceValue: Encodable, Sendable {
    case string(String)
    case int(Int)
    case double(Double)
    case bool(Bool)

    func encode(to encoder: any Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .string(let value): try container.encode(value)
        case .int(let value): try container.encode(value)
        case .double(let value): try container.encode(value)
        case .bool(let value): try container.encode(value)
        }
    }
}

extension TraceValue: ExpressibleByStringLiteral {
    init(stringLiteral value: String) { self = .string(value) }
}

extension TraceValue: ExpressibleByIntegerLiteral {
    init(integerLiteral value: Int) { self = .int(value) }
}

extension TraceValue: ExpressibleByFloatLiteral {
    init(floatLiteral value: Double) { self = .double(value) }
}

extension TraceValue: ExpressibleByBooleanLiteral {
    init(booleanLiteral value: Bool) { self = .bool(value) }
}

/// The fields of one event, in the order they are written.
///
/// An array rather than a dictionary because the order is part of what makes a
/// line readable, and because a pinned test compares the whole string.
typealias TraceFields = [(String, TraceValue)]

/// One event, shaped as `tracing-subscriber::fmt::json()` writes it.
///
/// `jp` writes this format under `JP_DEBUG=1` and the tooling already parses it,
/// so the app's timeline and jp's can be read together.
struct TraceLine {
    let timestamp: String
    let level: TraceLevel
    let target: String
    let fields: TraceFields
    let spans: [String]

    /// The line as it goes in the file, or `nil` if a value cannot be encoded.
    ///
    /// Assembled key by key rather than handed to `JSONEncoder` whole, because a
    /// keyed container writes its entries in an order Foundation chooses: the
    /// timestamp lands in the middle, and a duration ahead of the message it
    /// belongs to. Every scalar still goes through the encoder, so escaping is
    /// Foundation's job and not this file's.
    func encoded() -> String? {
        let encoder = JSONEncoder()

        guard
            let level = Self.encode(.string(level.rawValue), with: encoder),
            let target = Self.encode(.string(target), with: encoder),
            let timestamp = Self.encode(.string(timestamp), with: encoder),
            let fields = encodedFields(with: encoder)
        else {
            return nil
        }

        var line = "{\"timestamp\":\(timestamp),\"level\":\(level),\"target\":\(target),"
        line.append("\"fields\":\(fields)")

        // Omitted when empty: the parser treats the key as optional, and most
        // events are not nested inside anything.
        if !spans.isEmpty, let spans = encodedSpans(with: encoder) {
            line.append(",\"spans\":\(spans)")
        }

        line.append("}")
        return line
    }

    private func encodedFields(with encoder: JSONEncoder) -> String? {
        var entries: [String] = []
        entries.reserveCapacity(fields.count)

        for (name, value) in fields {
            guard
                let name = Self.encode(.string(name), with: encoder),
                let value = Self.encode(value, with: encoder)
            else {
                return nil
            }

            entries.append("\(name):\(value)")
        }

        return "{\(entries.joined(separator: ","))}"
    }

    private func encodedSpans(with encoder: JSONEncoder) -> String? {
        var entries: [String] = []
        entries.reserveCapacity(spans.count)

        for span in spans {
            guard let name = Self.encode(.string(span), with: encoder) else { return nil }
            entries.append("{\"name\":\(name)}")
        }

        return "[\(entries.joined(separator: ","))]"
    }

    /// One value as its JSON representation.
    private static func encode(_ value: TraceValue, with encoder: JSONEncoder) -> String? {
        guard let data = try? encoder.encode(value) else { return nil }
        return String(decoding: data, as: UTF8.self)
    }
}

/// The ratio turning `mach_absolute_time()` ticks into nanoseconds.
struct MachTimebase: Sendable {
    let numerator: UInt32
    let denominator: UInt32

    /// What this machine reports, read once.
    static let current: MachTimebase = {
        var info = mach_timebase_info_data_t()
        mach_timebase_info(&info)
        return MachTimebase(numerator: info.numer, denominator: info.denom)
    }()

    func nanoseconds(_ ticks: UInt64) -> UInt64 {
        ticks * UInt64(numerator) / UInt64(denominator)
    }
}

/// An append-only line sink, writable from any isolation domain.
///
/// `@unchecked Sendable` rather than an actor: an interval ends wherever the
/// work it timed ends, and an actor would put an `await` at every one of those
/// call sites, changing the timing being measured. The file handle is only ever
/// touched with `lock` held, which is what makes the unchecked claim true.
final class TraceWriter: @unchecked Sendable {
    /// The file being appended to.
    let url: URL

    private let lock = NSLock()
    private let handle: FileHandle

    /// Open `fileName` inside `directory`, creating both if they are missing.
    ///
    /// `nil` when no directory is given, which is how the app ships: nothing is
    /// created and nothing is written.
    init?(directory: URL?, fileName: String) {
        guard let directory else { return nil }

        let manager = FileManager.default
        let url = directory.appendingPathComponent(fileName)
        let path = url.path(percentEncoded: false)

        try? manager.createDirectory(at: directory, withIntermediateDirectories: true)
        if !manager.fileExists(atPath: path) {
            guard manager.createFile(atPath: path, contents: nil) else { return nil }
        }

        guard let handle = try? FileHandle(forWritingTo: url) else { return nil }
        _ = try? handle.seekToEnd()

        self.url = url
        self.handle = handle
    }

    deinit {
        try? handle.close()
    }

    /// Append `line` and a newline.
    ///
    /// A failed write is dropped rather than reported: the app is being observed,
    /// not driven by this, and a full disk is not a reason to interrupt what the
    /// person at the keyboard is reading.
    func append(_ line: String) {
        lock.withLock {
            try? handle.write(contentsOf: Data("\(line)\n".utf8))
        }
    }
}
