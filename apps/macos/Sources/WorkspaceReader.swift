import Foundation

/// A conversation, as `jp_workspace_conversations` reports it.
///
/// Hand-maintained to match `ConversationSummary` in the Rust `jp_plugin` crate.
/// Nothing checks that the two agree, so a field added there needs adding here
/// too; `ConversationSummaryTests` pins the payload this decodes from.
struct ConversationSummary: Decodable, Identifiable, Sendable, Equatable {
    /// The conversation ID, as a decisecond timestamp in decimal.
    let id: String

    /// The conversation title, absent until one has been generated or set.
    let title: String?

    /// When the conversation was last activated, as RFC 3339 text.
    ///
    /// Deliberately unparsed. The Rust side emits a fractional-seconds part
    /// whenever the stored timestamp has one, and `JSONDecoder`'s `.iso8601`
    /// strategy rejects fractional seconds, so a `Date` here would decode the
    /// whole-second case and fail on every real workspace. ``ConversationDate``
    /// is where the parsing happens, for the code that displays it.
    let lastActivatedAt: String

    /// When the conversation was pinned, as RFC 3339 text, absent if it is not
    /// pinned.
    ///
    /// Unparsed for the same reason as ``lastActivatedAt``, and nothing shows
    /// the instant itself: what the sidebar needs is ``isPinned``.
    let pinnedAt: String?

    /// How many events the conversation holds.
    let eventsCount: Int

    /// Whether the conversation is pinned.
    var isPinned: Bool {
        pinnedAt != nil
    }

    enum CodingKeys: String, CodingKey {
        case id
        case title
        case lastActivatedAt = "last_activated_at"
        case pinnedAt = "pinned_at"
        case eventsCount = "events_count"
    }
}

/// One piece of work the library timed inside a single call.
///
/// Hand-maintained to match `Span` in the Rust `jp_ffi` crate. Nothing checks
/// that the two agree; `WorkspaceReaderTests` decodes the exact payload that
/// crate's own tests pin.
struct LibrarySpan: Decodable, Sendable, Equatable {
    /// What the work is called, written as the trace event's message.
    let name: String

    /// How long it took, in milliseconds.
    let durationMS: Double

    enum CodingKeys: String, CodingKey {
        case name
        case durationMS = "duration_ms"
    }
}

/// A failure reported by the Rust library, or by decoding its output.
struct WorkspaceError: LocalizedError, Sendable, Equatable {
    let message: String

    var errorDescription: String? { message }
}

/// An open JP workspace.
///
/// Noncopyable, so the compiler enforces what the C contract requires: exactly
/// one owner of the handle, and exactly one `jp_workspace_close`. Copying this
/// would give two owners and a double free, which is a compile error rather than
/// a crash.
///
/// Reading takes locks and touches the filesystem. Call it off the main thread or
/// the UI stalls behind a slow read.
struct WorkspaceReader: ~Copyable {
    private let handle: OpaquePointer

    /// Open the workspace containing `path`, which may be the workspace root or
    /// any directory inside it.
    ///
    /// Opening writes to disk: it creates the user-local conversation store if
    /// missing and moves corrupt conversations aside, as `jp` does on startup.
    init(path: String) throws(WorkspaceError) {
        // Swift materializes a NUL-terminated buffer for the duration of the
        // call, which is all `jp_workspace_open` requires of the pointer.
        guard let handle = jp_workspace_open(path) else {
            throw Self.lastError()
        }
        self.handle = handle
    }

    deinit {
        jp_workspace_close(handle)
    }

    /// What a read is attributed to on the timeline.
    ///
    /// A target of its own, so time spent below this boundary reads as the
    /// library's rather than the app's.
    static let traceTarget = "JP.FFI"

    /// The interval a conversation-list read is recorded as.
    static let conversationsSpan = "workspace.conversations"

    /// The interval an event read is recorded as.
    static let eventsSpan = "workspace.events"

    /// Every conversation in the workspace, most recently active first.
    ///
    /// `spans` names the intervals already open around this call, root first.
    /// The library's own timings are recorded beneath them, so a reader sees
    /// where inside the app's work the library's time went.
    borrowing func conversations(
        spans: [String] = []
    ) throws(WorkspaceError) -> [ConversationSummary] {
        let timing = Trace.interval(
            Self.conversationsSpan, target: Self.traceTarget, spans: spans)
        defer { timing.end() }

        // Asked for only while something is listening. Unrecorded, the library
        // allocates no timings string and nothing here has one to release.
        var timings: UnsafeMutablePointer<CChar>?
        let raw =
            Trace.isRecording
            ? jp_workspace_conversations(handle, &timings)
            : jp_workspace_conversations(handle, nil)

        Self.record(timings, under: spans + [Self.conversationsSpan])

        guard let raw else {
            throw Self.lastError()
        }

        let json = Self.take(raw)
        do {
            return try JSONDecoder().decode([ConversationSummary].self, from: json)
        } catch {
            throw WorkspaceError(message: "could not decode the conversation list: \(error)")
        }
    }

    /// Every event in a conversation, oldest first.
    ///
    /// `conversationID` is the `id` of a summary from ``conversations(spans:)``.
    /// `spans` names the intervals already open around this call, root first.
    borrowing func events(
        for conversationID: String,
        spans: [String] = []
    ) throws(WorkspaceError) -> [ConversationTurn] {
        let timing = Trace.interval(Self.eventsSpan, target: Self.traceTarget, spans: spans)
        defer { timing.end() }

        var timings: UnsafeMutablePointer<CChar>?
        let raw =
            Trace.isRecording
            ? jp_workspace_events(handle, conversationID, &timings)
            : jp_workspace_events(handle, conversationID, nil)

        Self.record(timings, under: spans + [Self.eventsSpan])

        guard let raw else {
            throw Self.lastError()
        }

        do {
            return try JSONDecoder().decode([ConversationTurn].self, from: Self.take(raw))
        } catch {
            throw WorkspaceError(message: "could not decode the event list: \(error)")
        }
    }

    /// Write what the library timed inside one call, nested under `enclosing`.
    ///
    /// `raw` is null when no timings were asked for, and when the library could
    /// not build them.
    private static func record(
        _ raw: UnsafeMutablePointer<CChar>?,
        under enclosing: [String]
    ) {
        guard let raw else { return }

        for line in timingLines(take(raw), under: enclosing, at: Trace.timestamp(Date())) {
            Trace.write(line)
        }
    }

    /// The trace lines the library's timings become, nested under `enclosing`.
    ///
    /// Built rather than written straight out, so a test can pin them: nesting
    /// is the whole point of these events, and one written with an empty span
    /// stack looks like any other line in the file.
    ///
    /// Every span of one call carries `timestamp`, because durations are all the
    /// library reports. There is no second clock to place them on, and the order
    /// they are written in is the order they ran.
    ///
    /// A payload that will not decode produces no lines. Instrumentation nobody
    /// can read is not a reason to fail the read it was measuring.
    static func timingLines(
        _ json: Data,
        under enclosing: [String],
        at timestamp: String
    ) -> [String] {
        guard let spans = try? JSONDecoder().decode([LibrarySpan].self, from: json) else {
            return []
        }

        return spans.compactMap { span in
            Trace.line(
                timestamp: timestamp,
                level: .info,
                target: traceTarget,
                message: span.name,
                fields: [("duration_ms", .double(span.durationMS))],
                spans: enclosing
            )
        }
    }

    /// The library's message for the most recent failure on this thread.
    private static func lastError() -> WorkspaceError {
        guard let raw = jp_last_error() else {
            return WorkspaceError(message: "the library reported a failure without a message")
        }
        return WorkspaceError(message: String(decoding: take(raw), as: UTF8.self))
    }

    /// Copy a string the library allocated, releasing the original.
    ///
    /// Rust frees what Rust allocates, so the bytes are copied out and the
    /// pointer handed straight back.
    private static func take(_ raw: UnsafeMutablePointer<CChar>) -> Data {
        defer { jp_string_free(raw) }
        return Data(bytes: raw, count: strlen(raw))
    }
}
