import Foundation
import Testing

@testable import JP

@Suite("ConversationRef")
struct ConversationRefTests {
    private let reference = ConversationRef(
        workspacePath: "/tmp/my-workspace",
        conversationID: "17251488000",
        title: "Reading list"
    )

    /// The URI is the form JP itself uses to reference a conversation, so what
    /// lands on the pasteboard is something a person can paste into a query.
    ///
    /// This is what the `Transferable` conformance exports, and so what both a
    /// copy and a drag produce. The conformance itself is a single
    /// `ProxyRepresentation` over this property; driving it through
    /// `exported(as:)` needs an importable representation, which a reference has
    /// no use for until something can accept a drop.
    @Test("exports as a jp:// URI")
    func exportsAsAURI() {
        #expect(reference.uri == "jp://17251488000")
    }

    /// A window restored from disk has no title, and still needs one to show.
    @Test("falls back to the ID for a window title")
    func fallsBackToTheID() {
        let untitled = ConversationRef(
            workspacePath: "/tmp/my-workspace",
            conversationID: "17251488000"
        )

        #expect(untitled.displayTitle == "Conversation 17251488000")
    }

    @Test("prefers the title for a window title")
    func prefersTheTitle() {
        #expect(reference.displayTitle == "Reading list")
    }

    /// The system restores window values by encoding them, so a reference has to
    /// survive a round trip with the workspace path intact — that path is what
    /// lets a restored conversation window read its workspace without a
    /// workspace window open.
    @Test("survives the round trip the system restores windows through")
    func survivesARoundTrip() throws {
        let encoded = try JSONEncoder().encode(reference)
        let decoded = try JSONDecoder().decode(ConversationRef.self, from: encoded)

        #expect(decoded == reference)
        #expect(decoded.workspacePath == "/tmp/my-workspace")
    }
}
