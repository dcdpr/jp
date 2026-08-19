import Foundation

/// One event in a conversation, as `jp_workspace_events` presents it.
///
/// Hand-maintained to match `DisplayEvent` in the Rust `jp_ffi` crate. The `type`
/// tag names the *presentation*, not the stored event kind, so nothing here
/// decides what a `chat_request` means — that judgement is about the conversation
/// model and lives with the model.
///
/// Only messages reach this side. Tool calls, reasoning, inquiries and config
/// changes have no prose to show and the library leaves them out.
enum ConversationEvent: Decodable, Sendable, Equatable {
    /// A message the user sent, with the display name of whoever wrote it.
    case userMessage(timestamp: String, author: String?, text: String)

    /// A message the assistant replied with.
    case assistantMessage(timestamp: String, text: String)

    /// A presentation this build has no way to draw.
    ///
    /// Thrown rather than absorbed into a catch-all case, so the decision to
    /// skip it belongs to whoever is decoding a whole turn rather than being
    /// made silently here. A malformed event of a *known* presentation still
    /// fails, which is what keeps a wire-format mistake visible.
    struct UnknownPresentation: Error {
        let type: String
    }

    /// When the event was recorded, as RFC 3339 text.
    ///
    /// Kept unparsed because nothing displays it yet. Every timestamp the library
    /// reports uses this one format, so one decoder will cover events and
    /// conversation summaries alike when something needs it.
    var timestamp: String {
        switch self {
        case .userMessage(let timestamp, _, _),
            .assistantMessage(let timestamp, _):
            timestamp
        }
    }

    /// What the event has to say.
    var text: String {
        switch self {
        case .userMessage(_, _, let text),
            .assistantMessage(_, let text):
            text
        }
    }

    /// Who said it, as it should be shown above the message.
    ///
    /// A user message with no recorded author was written before a display name
    /// was configured, and is still theirs.
    var speaker: String {
        switch self {
        case .userMessage(_, let author, _): author ?? "You"
        case .assistantMessage: "Assistant"
        }
    }

    private enum CodingKeys: String, CodingKey {
        case type
        case timestamp
        case author
        case text
    }

    init(from decoder: any Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)

        // The tag is read before anything else, so an unrecognized presentation
        // throws `UnknownPresentation` rather than failing on a field it was
        // never going to have. Decoding a shared field up here would make
        // skipping such an event depend on it carrying that field.
        switch try container.decode(String.self, forKey: .type) {
        case "user_message":
            self = .userMessage(
                timestamp: try container.decode(String.self, forKey: .timestamp),
                author: try container.decodeIfPresent(String.self, forKey: .author),
                text: try container.decode(String.self, forKey: .text)
            )

        case "assistant_message":
            self = .assistantMessage(
                timestamp: try container.decode(String.self, forKey: .timestamp),
                text: try container.decode(String.self, forKey: .text)
            )

        case let type:
            throw UnknownPresentation(type: type)
        }
    }
}
