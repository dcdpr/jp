import Foundation
import Testing

@testable import JP

/// Decoding tests for the hand-maintained mirror of the Rust projection.
///
/// The payload here is copied verbatim from
/// `events_are_projected_as_turns_of_tagged_json` in
/// `crates/jp_ffi/src/lib_tests.rs`. When the Rust side changes shape, its test
/// fails and so does this one, which is the only link between the two
/// definitions.
@Suite("ConversationTurn decoding")
struct ConversationTurnDecodingTests {
    private func decode(_ json: String) throws -> [ConversationTurn] {
        try JSONDecoder().decode([ConversationTurn].self, from: Data(json.utf8))
    }

    @Test("decodes the payload the library emits")
    func decodesLibraryPayload() throws {
        let json = """
            [{"index":0,"events":[\
            {"type":"user_message","timestamp":"2024-09-01T10:00:01Z","author":"Jean",\
            "text":"What does this do?"},\
            {"type":"assistant_message","timestamp":"2024-09-01T10:00:03Z",\
            "text":"It reads conversations."}]}]
            """

        #expect(
            try decode(json) == [
                ConversationTurn(
                    index: 0,
                    events: [
                        .userMessage(
                            timestamp: "2024-09-01T10:00:01Z",
                            author: "Jean",
                            text: "What does this do?"
                        ),
                        .assistantMessage(
                            timestamp: "2024-09-01T10:00:03Z",
                            text: "It reads conversations."
                        ),
                    ]
                )
            ]
        )
    }

    /// The library numbers a turn by its place among all of them, so a turn it
    /// had nothing to show for leaves a gap. Two turns in a row can be 0 and 2.
    @Test("keeps the library's turn numbering, gaps and all")
    func keepsTheLibrarysNumbering() throws {
        let json = """
            [{"index":0,"events":[\
            {"type":"user_message","timestamp":"2024-09-01T10:00:00Z","text":"first"}]},\
            {"index":2,"events":[\
            {"type":"user_message","timestamp":"2024-09-01T10:00:02Z","text":"third"}]}]
            """

        #expect(try decode(json).map(\.index) == [0, 2])
    }

    /// A request authored before a display name was configured has no author,
    /// and is still shown as the user's.
    @Test("decodes a user message with no author")
    func decodesUserMessageWithoutAuthor() throws {
        let json = """
            [{"index":0,"events":[\
            {"type":"user_message","timestamp":"2024-09-01T10:00:00Z","text":"hi"}]}]
            """

        let turns = try decode(json)

        #expect(
            turns.first?.events == [
                .userMessage(timestamp: "2024-09-01T10:00:00Z", author: nil, text: "hi")
            ]
        )
        #expect(turns.first?.events.first?.speaker == "You")
    }

    /// A presentation added on the Rust side must not break an app built against
    /// the older shape: the event it cannot draw is left out and the rest of the
    /// turn still arrives.
    @Test("skips a presentation it does not know, keeping the rest of the turn")
    func skipsUnknownPresentation() throws {
        let json = """
            [{"index":0,"events":[\
            {"type":"some_future_presentation","timestamp":"2024-09-01T10:00:00Z"},\
            {"type":"user_message","timestamp":"2024-09-01T10:00:01Z","text":"hi"}]}]
            """

        #expect(
            try decode(json).first?.events == [
                .userMessage(timestamp: "2024-09-01T10:00:01Z", author: nil, text: "hi")
            ]
        )
    }

    /// A presentation this build cannot draw is skipped on the strength of its
    /// `type` alone, whatever else it does or does not carry. Reading any shared
    /// field before the tag would make skipping depend on that field being
    /// present, and an added presentation is exactly the thing least likely to
    /// share the shape of the two here.
    @Test("skips a presentation it does not know that shares no fields with one it does")
    func skipsUnknownPresentationWithNoCommonFields() throws {
        let json = """
            [{"index":0,"events":[\
            {"type":"tool_call","tool":"read_file","arguments":{}},\
            {"type":"user_message","timestamp":"2024-09-01T10:00:01Z","text":"hi"}]}]
            """

        #expect(
            try decode(json).first?.events == [
                .userMessage(timestamp: "2024-09-01T10:00:01Z", author: nil, text: "hi")
            ]
        )
    }

    /// Leniency stops at the `type` tag. A presentation this build *does* know,
    /// arriving without the fields it promises, is a wire-format mistake and
    /// fails rather than being quietly dropped.
    @Test("fails on a known presentation missing its fields")
    func failsOnMalformedKnownPresentation() {
        let json = """
            [{"index":0,"events":[\
            {"type":"user_message","timestamp":"2024-09-01T10:00:00Z"}]}]
            """

        #expect(throws: (any Error).self) {
            try decode(json)
        }
    }

    @Test("decodes an empty conversation")
    func decodesEmptyList() throws {
        #expect(try decode("[]").isEmpty)
    }

    @Test("reads the timestamp and speaker of either presentation")
    func readsTimestampAndSpeaker() throws {
        let json = """
            [{"index":0,"events":[\
            {"type":"user_message","timestamp":"2024-09-01T10:00:00Z","author":"Jean","text":"hi"},\
            {"type":"assistant_message","timestamp":"2024-09-01T10:00:01Z","text":"hello"}]}]
            """

        let events = try #require(decode(json).first?.events)

        #expect(events.map(\.timestamp) == ["2024-09-01T10:00:00Z", "2024-09-01T10:00:01Z"])
        #expect(events.map(\.speaker) == ["Jean", "Assistant"])
    }
}
