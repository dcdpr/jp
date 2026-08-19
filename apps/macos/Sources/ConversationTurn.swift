import Foundation

/// One turn of a conversation, as `jp_workspace_events` presents it.
///
/// Hand-maintained to match `DisplayTurn` in the Rust `jp_ffi` crate. A turn is
/// one user request through the assistant's final answer to it, and where its
/// boundaries fall is decided on the library side: the rules involve an
/// implicit leading turn and a marker that opens a turn only sometimes, neither
/// of which is recoverable from the events alone.
///
/// A turn the library had nothing to show for is absent rather than empty, so a
/// separator can be drawn between every pair of turns received.
struct ConversationTurn: Decodable, Sendable, Equatable, Identifiable {
    /// Where the turn sits in the conversation, counting from zero.
    ///
    /// The position among *all* turns, so the numbering skips any the library
    /// had nothing to show for. Two consecutive turns here can therefore be
    /// numbered 4 and 7.
    let index: Int

    /// What the turn has to show, oldest first.
    let events: [ConversationEvent]

    var id: Int { index }

    private enum CodingKeys: String, CodingKey {
        case index
        case events
    }

    init(index: Int, events: [ConversationEvent]) {
        self.index = index
        self.events = events
    }

    init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)

        index = try container.decode(Int.self, forKey: .index)
        events = try container.decode([SkippableEvent].self, forKey: .events)
            .compactMap(\.event)
    }
}

/// An event that decodes to nothing when this build cannot draw it.
///
/// A later library adding a presentation — a tool call, an attachment — would
/// otherwise fail the whole conversation on an app that predates it. Only an
/// unrecognized `type` is skipped; a known presentation missing its fields
/// still throws.
private struct SkippableEvent: Decodable {
    let event: ConversationEvent?

    init(from decoder: any Decoder) throws {
        do {
            event = try ConversationEvent(from: decoder)
        } catch is ConversationEvent.UnknownPresentation {
            event = nil
        }
    }
}
