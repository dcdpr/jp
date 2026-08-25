import ApplicationServices
import Testing

@testable import DriveKit

@Suite("Act.type")
struct TypeTests {
    /// A text field shaped like the one SwiftUI produces: `AXValue` writable,
    /// `AXPress` absent.
    private func field(identifier: String = "sidebar.filter") -> FakeElement {
        return FakeElement(
            role: "AXTextField",
            identifier: identifier,
            settable: [kAXValueAttribute, kAXFocusedAttribute],
            actions: ["AXShowMenu", "AXConfirm"]
        )
    }

    @Test("writes the text into the field")
    func writesTheText() throws {
        let field = field()
        let root = FakeElement(role: "AXApplication", children: [field])

        let result = try Act.run(
            .type(.init(identifier: "sidebar.filter", text: "driving")),
            in: root
        )

        #expect(field.attributes[kAXValueAttribute] == "driving")
        #expect(result.step == "type")
        #expect(result.role == "AXTextField")
        #expect(result.confirmed == true)
    }

    /// Writing the value alone changes the text a `SwiftUI` field shows without the
    /// binding behind it noticing, so the application carries on as though nothing
    /// was typed. The confirm is what the application actually observes, and a
    /// `type` that skipped it would report success having done nothing.
    @Test("commits the edit through the field's confirm action")
    func commitsTheEdit() throws {
        let field = field()
        let root = FakeElement(role: "AXApplication", children: [field])

        let result = try Act.run(
            .type(.init(identifier: "sidebar.filter", text: "driving")),
            in: root
        )

        #expect(field.performed == ["AXConfirm"])
        #expect(result.committed == true)
    }

    /// A field that publishes every change as it happens needs nothing committing,
    /// so this is reported rather than treated as a failure.
    @Test("reports a field with no confirm action as uncommitted")
    func reportsAnUncommittedWrite() throws {
        let field = FakeElement(
            role: "AXTextField",
            identifier: "live",
            settable: [kAXValueAttribute],
            actions: []
        )
        let root = FakeElement(role: "AXApplication", children: [field])

        let result = try Act.run(.type(.init(identifier: "live", text: "x")), in: root)

        #expect(result.confirmed == true)
        #expect(result.committed == false)
        #expect(field.performed.isEmpty)
    }

    /// Text in the field that the application never saw is the worst outcome to
    /// report as success, so a refused confirm fails the step.
    @Test("fails when the edit cannot be committed")
    func failsOnARefusedConfirm() throws {
        let field = field()
        field.performStatus = .cannotComplete
        let root = FakeElement(role: "AXApplication", children: [field])

        let error = try #require(throws: DriveError.self) {
            try Act.run(.type(.init(identifier: "sidebar.filter", text: "x")), in: root)
        }

        #expect(error.kind == .actionFailed)
        #expect(error.hint?.contains("not committed") == true)
    }

    /// Typing replaces rather than appends, so a script does not have to clear the
    /// field first and a second step cannot silently concatenate.
    @Test("replaces what the field already held")
    func replacesExistingText() throws {
        let field = field()
        field.attributes[kAXValueAttribute] = "old"
        let root = FakeElement(role: "AXApplication", children: [field])

        _ = try Act.run(.type(.init(identifier: "sidebar.filter", text: "new")), in: root)

        #expect(field.attributes[kAXValueAttribute] == "new")
    }

    /// Clearing is typing nothing, not a step of its own.
    @Test("an empty string clears the field")
    func clearsTheField() throws {
        let field = field()
        field.attributes[kAXValueAttribute] = "something"
        let root = FakeElement(role: "AXApplication", children: [field])

        let result = try Act.run(.type(.init(identifier: "sidebar.filter", text: "")), in: root)

        #expect(field.attributes[kAXValueAttribute] == "")
        #expect(result.confirmed == true)
    }

    /// A static label and a disabled field both resolve by identifier and both
    /// refuse the write. Failing here beats reporting a write that went nowhere.
    @Test("fails on an element whose value is not writable")
    func failsOnAReadOnlyElement() throws {
        let label = FakeElement(role: "AXStaticText", identifier: "subtitle")
        let root = FakeElement(role: "AXApplication", children: [label])

        let error = try #require(throws: DriveError.self) {
            try Act.run(.type(.init(identifier: "subtitle", text: "x")), in: root)
        }

        #expect(error.kind == .notEditable)
        #expect(label.attributes[kAXValueAttribute] == nil)
    }

    @Test("reports a refused write")
    func reportsARefusedWrite() throws {
        let field = field()
        field.writeStatus = .cannotComplete
        let root = FakeElement(role: "AXApplication", children: [field])

        let error = try #require(throws: DriveError.self) {
            try Act.run(.type(.init(identifier: "sidebar.filter", text: "x")), in: root)
        }

        #expect(error.kind == .writeFailed)
        #expect(error.message.contains("cannot_complete"))
    }

    /// The accessibility API lets a target accept a write and discard it, which is
    /// the whole reason the step reads back instead of trusting the status.
    @Test("reports an accepted write that did not take")
    func reportsAnIneffectiveWrite() throws {
        let field = field()
        field.ignoresWrites = true
        let root = FakeElement(role: "AXApplication", children: [field])

        let result = try Act.run(
            .type(.init(identifier: "sidebar.filter", text: "driving")),
            in: root
        )

        #expect(
            result.confirmed == false,
            "a field that discarded the text must not report as confirmed"
        )
    }

    @Test("reports an identifier it could not find")
    func reportsAMissingIdentifier() throws {
        let root = FakeElement(role: "AXApplication", children: [field()])

        let error = try #require(throws: DriveError.self) {
            try Act.run(.type(.init(identifier: "nope", text: "x")), in: root)
        }

        #expect(error.kind == .identifierNotFound)
    }

    /// Only the addressed field is written to, so a step cannot quietly clobber a
    /// second field that happens to sit nearby.
    @Test("leaves other fields alone")
    func leavesOtherFieldsAlone() throws {
        let first = field(identifier: "one")
        let second = field(identifier: "two")
        let root = FakeElement(role: "AXApplication", children: [first, second])

        _ = try Act.run(.type(.init(identifier: "two", text: "x")), in: root)

        #expect(first.attributes[kAXValueAttribute] == nil)
        #expect(second.attributes[kAXValueAttribute] == "x")
    }
}
