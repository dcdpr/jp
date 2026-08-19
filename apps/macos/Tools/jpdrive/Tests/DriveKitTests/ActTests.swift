import ApplicationServices
import Testing

@testable import DriveKit

@Suite("Act")
struct ActTests {
    /// The case the driver exists for, and the one that was broken: the identifier
    /// is on a leaf two levels below the element that owns selection.
    @Test("select writes AXSelected on the row, not on the identified element")
    func selectsTheOwningRow() throws {
        let root = FakeElement.sidebar(rowCount: 3)
        let step = Step.select(.init(identifier: "sidebar.row.1"))

        let result = try Act.run(step, in: root)

        #expect(
            result
                == StepResult(
                    step: "select",
                    identifier: "sidebar.row.1",
                    role: "AXRow",
                    confirmed: true
                )
        )
        #expect(result.confirmed == true)

        let rows = try #require(root.children.first?.children)
        #expect(rows[1].attributes[kAXSelectedAttribute] == "1")

        // The leaf that carried the identifier must not have been written to. It
        // reports no AXSelected at all, and a driver that wrote there would report
        // success while selecting nothing.
        let leaf = try #require(rows[1].children.first?.children.first)
        #expect(leaf.attributes[kAXSelectedAttribute] == nil)
    }

    /// Selection reaches a row regardless of where it sits, which is what makes the
    /// attribute write preferable to a synthesized click.
    @Test("select reaches a row far down a long list")
    func selectsADeepRow() throws {
        let root = FakeElement.sidebar(rowCount: 1000)

        let result = try Act.run(.select(.init(identifier: "sidebar.row.987")), in: root)

        #expect(result.confirmed == true)
        let rows = try #require(root.children.first?.children)
        #expect(rows[987].attributes[kAXSelectedAttribute] == "1")
    }

    @Test("select reports the identifier it could not find")
    func reportsAMissingIdentifier() {
        let root = FakeElement.sidebar(rowCount: 3)

        #expect(throws: DriveError.self) {
            try Act.run(.select(.init(identifier: "sidebar.row.nope")), in: root)
        }
    }

    /// An element nothing in its chain can select is a failure, not a fallback onto
    /// some other mechanism.
    @Test("select fails when no ancestor accepts the write")
    func failsWhenNothingIsSelectable() throws {
        let leaf = FakeElement(role: "AXUnknown", identifier: "lonely")
        let root = FakeElement(role: "AXApplication", children: [leaf])

        let error = try #require(throws: DriveError.self) {
            try Act.run(.select(.init(identifier: "lonely")), in: root)
        }

        #expect(error.kind == .notSelectable)
    }

    /// A write the accessibility API refuses is reported, not silently treated as
    /// an unconfirmed success.
    @Test("select reports a refused write")
    func reportsARefusedWrite() throws {
        let root = FakeElement.sidebar(rowCount: 2)
        let rows = try #require(root.children.first?.children)
        rows[0].writeStatus = .cannotComplete

        let error = try #require(throws: DriveError.self) {
            try Act.run(.select(.init(identifier: "sidebar.row.0")), in: root)
        }

        #expect(error.kind == .writeFailed)
        #expect(error.message.contains("cannot_complete"))
    }

    /// A write can be accepted and do nothing. The step reports that as an
    /// unconfirmed success rather than as a failure, because the distinction is
    /// what tells a caller the mechanism stopped working.
    @Test("select reports an accepted write that changed nothing")
    func reportsAnIneffectiveWrite() throws {
        let leaf = FakeElement(role: "AXUnknown", identifier: "row")
        let row = FakeElement(role: "AXRow", settable: [kAXSelectedAttribute], children: [leaf])
        let root = FakeElement(role: "AXApplication", children: [row])
        row.ignoresWrites = true

        let result = try Act.run(.select(.init(identifier: "row")), in: root)

        #expect(result.role == "AXRow")
        #expect(
            result.confirmed == false,
            "a write the target discarded must not report as confirmed"
        )
    }

    /// A sidebar row is the case that makes `click` the wrong tool: the identified
    /// element has no activation point, and `select` reaches it whether or not it
    /// is on screen.
    @Test("click fails on a row, which wants select instead")
    func clickFailsOnARow() throws {
        let root = FakeElement.sidebar(rowCount: 1)

        let error = try #require(throws: DriveError.self) {
            try Act.run(
                .click(.init(identifier: "sidebar.row.0")), in: root, poster: FakePoster())
        }

        #expect(error.kind == .notClickable)
    }

    @Test("press performs AXPress on the identified element")
    func pressesTheElement() throws {
        let item = FakeElement(
            role: "AXMenuItem",
            identifier: "terminate:",
            actions: ["AXCancel", "AXPress", "AXPick"]
        )
        let root = FakeElement(role: "AXApplication", children: [item])

        let result = try Act.run(.press(.init(identifier: "terminate:")), in: root)

        #expect(item.performed == ["AXPress"])
        #expect(result.step == "press")
        #expect(result.role == "AXMenuItem")
    }

    /// Nothing readable says a press worked, so the step must not claim it did.
    /// Reporting `true` here would be the one dishonest field in the output.
    @Test("press reports no confirmation")
    func pressDoesNotClaimConfirmation() throws {
        let item = FakeElement(role: "AXButton", identifier: "go", actions: ["AXPress"])
        let root = FakeElement(role: "AXApplication", children: [item])

        let result = try Act.run(.press(.init(identifier: "go")), in: root)

        #expect(result.confirmed == nil)
    }

    /// A sidebar row is the case this catches: it advertises no actions at all, so
    /// the error points at the step that does work on it.
    @Test("press fails on an element that does not accept it")
    func pressFailsWithoutTheAction() throws {
        let root = FakeElement.sidebar(rowCount: 1)

        let error = try #require(throws: DriveError.self) {
            try Act.run(.press(.init(identifier: "sidebar.row.0")), in: root)
        }

        #expect(error.kind == .actionUnsupported)
        #expect(error.hint?.contains("select") == true)
        // The press must not have been attempted anyway.
        let leaf = root.children.first?.children.first?.children.first?.children.first
        #expect(leaf?.performed.isEmpty == true)
    }

    /// An element with other actions gets told what it does accept, which is how a
    /// script author finds the right verb without dumping the tree.
    @Test("press names the actions an element does accept")
    func pressNamesAvailableActions() throws {
        let item = FakeElement(role: "AXRow", identifier: "row", actions: ["AXShowMenu"])
        let root = FakeElement(role: "AXApplication", children: [item])

        let error = try #require(throws: DriveError.self) {
            try Act.run(.press(.init(identifier: "row")), in: root)
        }

        #expect(error.hint?.contains("AXShowMenu") == true)
    }

    /// `press` is a shorthand for `perform` with `AXPress`, and must keep saying so
    /// in its result rather than reporting the mechanism underneath.
    @Test("press names itself, not the general mechanism")
    func pressNamesItself() throws {
        let item = FakeElement(role: "AXButton", identifier: "go", actions: ["AXPress"])
        let root = FakeElement(role: "AXApplication", children: [item])

        let result = try Act.run(.press(.init(identifier: "go")), in: root)

        #expect(result.step == "press")
    }

    /// The escape hatch for the actions with no step of their own. A text field
    /// offers `AXConfirm` and no `AXPress`, so this is the only way to reach it.
    @Test("perform runs any action the element advertises")
    func performsANamedAction() throws {
        let field = FakeElement(
            role: "AXTextField",
            identifier: "sidebar.filter",
            actions: ["AXShowMenu", "AXConfirm"]
        )
        let root = FakeElement(role: "AXApplication", children: [field])

        let result = try Act.run(
            .perform(.init(identifier: "sidebar.filter", action: "AXConfirm")),
            in: root
        )

        #expect(field.performed == ["AXConfirm"])
        #expect(result.step == "perform")
    }

    @Test("perform fails on an action the element does not advertise")
    func performRejectsAnUnknownAction() throws {
        let field = FakeElement(
            role: "AXTextField",
            identifier: "sidebar.filter",
            actions: ["AXConfirm"]
        )
        let root = FakeElement(role: "AXApplication", children: [field])

        let error = try #require(throws: DriveError.self) {
            try Act.run(
                .perform(.init(identifier: "sidebar.filter", action: "AXPress")),
                in: root
            )
        }

        #expect(error.kind == .actionUnsupported)
        #expect(error.hint == "it accepts: AXConfirm")
        #expect(field.performed.isEmpty)
    }

    @Test("press reports a refused action")
    func pressReportsARefusal() throws {
        let item = FakeElement(role: "AXButton", identifier: "go", actions: ["AXPress"])
        item.performStatus = .cannotComplete
        let root = FakeElement(role: "AXApplication", children: [item])

        let error = try #require(throws: DriveError.self) {
            try Act.run(.press(.init(identifier: "go")), in: root)
        }

        #expect(error.kind == .actionFailed)
        #expect(error.message.contains("cannot_complete"))
    }
}
