import ApplicationServices
import Testing

@testable import DriveKit

@Suite("Act.click")
struct ClickTests {
    /// A button inside a window, which is the shape a click needs: something with a
    /// point, under something that can be raised.
    private func app(activationPoint: CGPoint? = CGPoint(x: 120, y: 340)) -> FakeElement {
        let button = FakeElement(
            role: "AXButton", identifier: "toolbar.open", actions: ["AXPress"])
        if let activationPoint {
            button.points[AXElement.activationPoint] = activationPoint
        }

        let window = FakeElement(
            role: kAXWindowRole,
            identifier: "workspace-AppWindow-1",
            actions: [kAXRaiseAction],
            children: [button]
        )

        return FakeElement(role: "AXApplication", children: [window])
    }

    @Test("clicks where the element says a click belongs")
    func clicksTheActivationPoint() throws {
        let root = app()
        let poster = FakePoster()

        let result = try Act.run(
            .click(.init(identifier: "toolbar.open")),
            in: root,
            poster: poster
        )

        #expect(poster.clicks == [CGPoint(x: 120, y: 340)])
        #expect(result.step == "click")
        #expect(result.role == "AXButton")
        #expect(result.point == "120.0,340.0")
    }

    /// `AXRaise` orders a window within its own application; ordering *between*
    /// applications follows activation. The driver is invoked from a terminal, so
    /// without this the terminal stays frontmost and the click lands there while
    /// the result names the element it aimed at.
    @Test("brings the application forward before clicking")
    func activatesTheApplication() throws {
        let root = app()

        _ = try Act.run(
            .click(.init(identifier: "toolbar.open")), in: root, poster: FakePoster())

        #expect(root.flag(kAXFrontmostAttribute) == true)
    }

    /// Posting into another application's window is worse than not clicking, so a
    /// refused activation stops the step rather than aiming anyway.
    @Test("refuses to click when the application cannot be brought forward")
    func refusesToClickWithoutActivating() throws {
        let root = app()
        root.writeStatus = .cannotComplete
        let poster = FakePoster()

        let error = try #require(throws: DriveError.self) {
            try Act.run(.click(.init(identifier: "toolbar.open")), in: root, poster: poster)
        }

        #expect(error.kind == .writeFailed)
        #expect(poster.clicks.isEmpty, "nothing may be posted at a background application")
    }

    /// The event goes to whatever occupies the coordinate, so a window behind
    /// another one would have its click swallowed. Raising is what makes the
    /// coordinate mean the element that named it.
    @Test("raises the window before clicking")
    func raisesTheWindowFirst() throws {
        let root = app()
        let window = try #require(root.children.first)

        _ = try Act.run(
            .click(.init(identifier: "toolbar.open")), in: root, poster: FakePoster())

        #expect(window.performed == [kAXRaiseAction])
    }

    /// A sidebar row has no activation point of its own, and pointing at `select`
    /// is more use than clicking at the origin would be.
    @Test("fails on an element with nowhere to click")
    func failsWithoutAnActivationPoint() throws {
        let poster = FakePoster()

        let error = try #require(throws: DriveError.self) {
            try Act.run(
                .click(.init(identifier: "toolbar.open")),
                in: app(activationPoint: nil),
                poster: poster
            )
        }

        #expect(error.kind == .notClickable)
        #expect(error.hint?.contains("select") == true)
        #expect(poster.clicks.isEmpty, "nothing may be clicked when there is no point to click")
    }

    @Test("reports a click that could not be posted")
    func reportsAFailedPost() throws {
        let poster = FakePoster()
        poster.succeeds = false

        let error = try #require(throws: DriveError.self) {
            try Act.run(.click(.init(identifier: "toolbar.open")), in: app(), poster: poster)
        }

        #expect(error.kind == .actionFailed)
        #expect(error.message.contains("120.0,340.0"))
    }

    @Test("reports an identifier it could not find")
    func reportsAMissingIdentifier() throws {
        let poster = FakePoster()

        let error = try #require(throws: DriveError.self) {
            try Act.run(.click(.init(identifier: "nope")), in: app(), poster: poster)
        }

        #expect(error.kind == .identifierNotFound)
        #expect(poster.clicks.isEmpty)
    }

    /// An element outside any window still has a point, and clicking it is better
    /// than refusing because there was nothing to raise.
    @Test("clicks without a window to raise")
    func clicksWithoutAWindow() throws {
        let element = FakeElement(role: "AXButton", identifier: "loose")
        element.points[AXElement.activationPoint] = CGPoint(x: 1, y: 2)
        let root = FakeElement(role: "AXApplication", children: [element])
        let poster = FakePoster()

        _ = try Act.run(.click(.init(identifier: "loose")), in: root, poster: poster)

        #expect(poster.clicks == [CGPoint(x: 1, y: 2)])
    }
}
