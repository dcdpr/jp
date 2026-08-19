import ApplicationServices
import Testing

@testable import DriveKit

@Suite("Act.drag")
struct DragTests {
    /// A window with a known frame, which is all a drag needs: somewhere to
    /// measure fractions against, and something to raise.
    private func app(
        origin: CGPoint? = CGPoint(x: 100, y: 200),
        size: CGSize? = CGSize(width: 800, height: 600)
    ) -> FakeElement {
        let window = FakeElement(
            role: kAXWindowRole,
            identifier: "workspace-AppWindow-1",
            actions: [kAXRaiseAction]
        )
        if let origin {
            window.points[kAXPositionAttribute] = origin
        }
        if let size {
            window.sizes[kAXSizeAttribute] = size
        }

        return FakeElement(role: "AXApplication", children: [window])
    }

    private func step(
        from: (Double, Double),
        to: (Double, Double),
        steps: Int? = nil,
        pauseMs: Int? = nil
    ) -> Step {
        .drag(
            .init(
                identifier: "workspace-AppWindow-1",
                from: .init(dx: from.0, dy: from.1),
                to: .init(dx: to.0, dy: to.1),
                steps: steps,
                pauseMs: pauseMs
            )
        )
    }

    /// Fractions are resolved against the element's own frame, so a script says
    /// "the right edge, halfway down" rather than a screen coordinate that stops
    /// being right the moment the window moves.
    @Test("resolves fractional offsets against the element's frame")
    func resolvesOffsets() throws {
        let poster = FakePoster()

        _ = try Act.run(
            step(from: (1.0, 0.5), to: (0.5, 0.5), steps: 2), in: app(), poster: poster)

        // Right edge, halfway down: 100 + 800, 200 + 300. Halfway across: 100 + 400.
        #expect(
            poster.drags == [
                [
                    CGPoint(x: 900, y: 500),
                    CGPoint(x: 700, y: 500),
                    CGPoint(x: 500, y: 500),
                ]
            ]
        )
    }

    /// The step exists to produce many frames rather than one jump, so the count
    /// is asserted rather than assumed: a drag delivered as a single move cannot
    /// show what a view does *during* a gesture, which is the whole reason for it.
    @Test("posts one move per step, plus the press")
    func postsOneMovePerStep() throws {
        let poster = FakePoster()

        let result = try Act.run(
            step(from: (0, 0), to: (1, 0), steps: 12), in: app(), poster: poster)

        #expect(poster.drags.first?.count == 13)
        #expect(result.moves == 12)
        #expect(result.step == "drag")
        #expect(result.role == kAXWindowRole)
    }

    @Test("defaults to enough moves to be a gesture")
    func defaultsToAGesture() throws {
        let poster = FakePoster()

        _ = try Act.run(step(from: (0, 0), to: (1, 1)), in: app(), poster: poster)

        #expect(poster.drags.first?.count == 25)
        #expect(poster.pauses == [.milliseconds(8)])
    }

    @Test("honours a stated pause between moves")
    func honoursThePause() throws {
        let poster = FakePoster()

        _ = try Act.run(
            step(from: (0, 0), to: (1, 1), steps: 3, pauseMs: 40), in: app(), poster: poster)

        #expect(poster.pauses == [.milliseconds(40)])
    }

    /// A drag of zero steps is a click with extra words. Clamped rather than
    /// rejected, so a caller computing the count from a distance cannot produce a
    /// path with nothing in it.
    @Test("clamps a step count below one")
    func clampsZeroSteps() throws {
        let poster = FakePoster()

        _ = try Act.run(
            step(from: (0, 0), to: (1, 1), steps: 0), in: app(), poster: poster)

        #expect(poster.drags.first?.count == 2)
    }

    /// The events land on whatever occupies the coordinates, so a window behind
    /// another would have the gesture swallowed.
    @Test("raises the window before dragging")
    func raisesTheWindowFirst() throws {
        let root = app()
        let window = try #require(root.children.first)

        _ = try Act.run(step(from: (1, 0.5), to: (0.5, 0.5)), in: root, poster: FakePoster())

        #expect(window.performed == [kAXRaiseAction])
    }

    /// Raising alone is not enough, and this is the assertion that says so.
    ///
    /// `AXRaise` orders a window forward within its own application; the ordering
    /// *between* applications follows activation. A drag posted at a background
    /// window's coordinates without this was received by the frontmost terminal
    /// instead — measured, and the reason the step takes focus.
    @Test("brings the application forward before dragging")
    func activatesTheApplication() throws {
        let root = app()

        _ = try Act.run(step(from: (1, 0.5), to: (0.5, 0.5)), in: root, poster: FakePoster())

        #expect(root.flag(kAXFrontmostAttribute) == true)
    }

    /// A tree that is not a running application has nothing to activate, and a
    /// gesture against one is still worth posting: every other assertion in this
    /// file depends on that.
    @Test("drags even when the application cannot be brought forward")
    func dragsWithoutActivating() throws {
        let root = app()
        root.writeStatus = .cannotComplete
        let poster = FakePoster()

        _ = try Act.run(step(from: (0, 0), to: (1, 1), steps: 2), in: root, poster: poster)

        #expect(poster.drags.first?.count == 3)
    }

    @Test("fails on an element with no frame to measure")
    func failsWithoutAFrame() throws {
        let poster = FakePoster()

        let error = try #require(throws: DriveError.self) {
            try Act.run(step(from: (0, 0), to: (1, 1)), in: app(size: nil), poster: poster)
        }

        #expect(error.kind == .notClickable)
        #expect(poster.drags.isEmpty, "nothing may be dragged across a frame that is not known")
    }

    @Test("reports a drag that could not be posted")
    func reportsAFailedPost() throws {
        let poster = FakePoster()
        poster.succeeds = false

        let error = try #require(throws: DriveError.self) {
            try Act.run(step(from: (0, 0), to: (1, 1)), in: app(), poster: poster)
        }

        #expect(error.kind == .actionFailed)
        #expect(error.message.contains("100.0,200.0"))
    }

    @Test("reports an identifier it could not find")
    func reportsAMissingIdentifier() throws {
        let poster = FakePoster()

        let error = try #require(throws: DriveError.self) {
            try Act.run(
                .drag(
                    .init(
                        identifier: "nope",
                        from: .init(dx: 0, dy: 0),
                        to: .init(dx: 1, dy: 1),
                        steps: nil,
                        pauseMs: nil
                    )
                ),
                in: app(),
                poster: poster
            )
        }

        #expect(error.kind == .identifierNotFound)
        #expect(poster.drags.isEmpty)
    }

    /// Decoded from the wire, because the snake-cased key is spelled by hand and a
    /// mismatch there reads as the default silently applying.
    @Test("decodes a step from its written form")
    func decodesFromJSON() throws {
        let json = """
            {"drag": {"identifier": "transcript.text", "from": {"dx": 0.1, "dy": 0.2},
             "to": {"dx": 0.8, "dy": 0.6}, "steps": 6, "pause_ms": 15}}
            """

        let decoded = try JSONDecoder().decode(Step.self, from: Data(json.utf8))

        guard case .drag(let target) = decoded else {
            Issue.record("expected a drag step, got \(decoded)")
            return
        }

        #expect(target.identifier == "transcript.text")
        #expect(target.from.dx == 0.1)
        #expect(target.to.dy == 0.6)
        #expect(target.steps == 6)
        #expect(target.pauseMs == 15)
    }
}
