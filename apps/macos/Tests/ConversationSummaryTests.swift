import Foundation
import Testing

@testable import JP

/// Decoding tests for the hand-maintained mirror of the Rust payload.
///
/// The payloads here are copied verbatim from the assertions in
/// `crates/jp_ffi/src/lib_tests.rs`. When the Rust side changes shape, its tests
/// fail and so do these, which is the only link between the two definitions.
@Suite("ConversationSummary decoding")
struct ConversationSummaryTests {
    private func decode(_ json: String) throws -> [ConversationSummary] {
        try JSONDecoder().decode([ConversationSummary].self, from: Data(json.utf8))
    }

    @Test("decodes the payload the library emits")
    func decodesLibraryPayload() throws {
        let json = """
            [{"id":"17251488000","title":"Reading list",\
            "last_activated_at":"2024-09-02T12:30:00Z","events_count":0}]
            """

        let decoded = try decode(json)

        #expect(
            decoded == [
                ConversationSummary(
                    id: "17251488000",
                    title: "Reading list",
                    lastActivatedAt: "2024-09-02T12:30:00Z",
                    pinnedAt: nil,
                    eventsCount: 0
                )
            ]
        )
    }

    /// The key is present only for a pinned conversation, so its absence is what
    /// says a conversation is not pinned.
    @Test("decodes the payload a pinned conversation emits")
    func decodesPinnedConversation() throws {
        let json = """
            [{"id":"17251488000","title":"Reading list",\
            "last_activated_at":"2024-09-02T12:30:00Z",\
            "pinned_at":"2024-09-03T08:00:00Z","events_count":0}]
            """

        let decoded = try decode(json)

        #expect(decoded.first?.pinnedAt == "2024-09-03T08:00:00Z")
        #expect(decoded.first?.isPinned == true)
    }

    @Test("decodes a missing pin as not pinned")
    func decodesMissingPin() throws {
        let json = """
            [{"id":"17251488000","title":"Reading list",\
            "last_activated_at":"2024-09-02T12:30:00Z","events_count":0}]
            """

        let decoded = try decode(json)

        #expect(decoded.first?.pinnedAt == nil)
        #expect(decoded.first?.isPinned == false)
    }

    /// Any conversation JP created from a wall clock carries sub-second
    /// precision, so this is the shape the app sees in practice. A `Date` field
    /// using `JSONDecoder`'s `.iso8601` strategy would fail here while passing
    /// the whole-second case above.
    @Test("decodes a timestamp with fractional seconds")
    func decodesFractionalSeconds() throws {
        let json = """
            [{"id":"17251488000","title":"Reading list",\
            "last_activated_at":"2024-09-02T12:30:00.123456Z","events_count":0}]
            """

        let decoded = try decode(json)

        #expect(decoded.first?.lastActivatedAt == "2024-09-02T12:30:00.123456Z")
    }

    /// A conversation keeps no title until one is generated or set.
    @Test("decodes a missing title as nil")
    func decodesMissingTitle() throws {
        let json = """
            [{"id":"17251488000","last_activated_at":"2024-09-02T12:30:00Z","events_count":3}]
            """

        let decoded = try decode(json)

        #expect(decoded.first?.title == nil)
        #expect(decoded.first?.eventsCount == 3)
    }

    /// A field added on the Rust side must not break an app built against the
    /// older shape, so unknown keys are ignored rather than rejected.
    @Test("ignores fields it does not know")
    func ignoresUnknownFields() throws {
        let json = """
            [{"id":"17251488000","title":"Reading list",\
            "last_activated_at":"2024-09-02T12:30:00Z","events_count":0,\
            "some_field_from_a_newer_library":true}]
            """

        let decoded = try decode(json)

        #expect(decoded.count == 1)
    }

    @Test("decodes an empty workspace")
    func decodesEmptyList() throws {
        #expect(try decode("[]").isEmpty)
    }

    /// The list is keyed by `id` in SwiftUI, so two conversations must not
    /// collide.
    @Test("uses the conversation ID as its identity")
    func identityIsTheConversationID() throws {
        let json = """
            [{"id":"17251488000","title":"A","last_activated_at":"2024-09-02T12:30:00Z",\
            "events_count":0},\
            {"id":"17251488001","title":"B","last_activated_at":"2024-09-02T12:30:00Z",\
            "events_count":0}]
            """

        let decoded = try decode(json)

        #expect(decoded.map(\.id) == ["17251488000", "17251488001"])
    }
}
